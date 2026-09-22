use crate::bundle::embedded::ASSET_VERSION;
use crate::http::overlay::canvas_specification;
use crate::server::state::{EditorConnection, Shared};
use crate::websocket::backpressure::send_text;
use crate::websocket::hub::stage_snapshot;
use crate::websocket::protocol::{StageControlEnvelope, version_mismatch};
use axum::{
    extract::{Path, RawQuery, State, WebSocketUpgrade, ws::Message},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
pub(crate) fn display_state(
    state: scorepeek_overlay::OverlayState,
    sample: bool,
) -> scorepeek_overlay::OverlayState {
    if sample && state.system == scorepeek_overlay::LampState::Inactive {
        scorepeek_overlay::editor_sample_state()
    } else {
        state
    }
}

pub(crate) async fn handle_canvas_socket_message(
    socket: &mut axum::extract::ws::WebSocket,
    message: Option<Result<Message, axum::Error>>,
    id: &str,
) -> bool {
    let Some(Ok(Message::Text(text))) = message else {
        return !matches!(message, None | Some(Err(_) | Ok(Message::Close(_))));
    };
    if let Ok(message) = serde_json::from_str::<serde_json::Value>(&text)
        && message.get("type").and_then(serde_json::Value::as_str) == Some("skin_diagnostic")
    {
        crate::diagnostics::emit(
            "skin_render",
            &serde_json::json!({
                "backend":"obs",
                "canvas_id":id,
                "status":message.get("status").and_then(serde_json::Value::as_str).filter(|value|matches!(*value,"success"|"failed")).unwrap_or("failed"),
                "phase":message.get("phase").and_then(serde_json::Value::as_str).filter(|value|matches!(*value,"init"|"render")),
                "duration_us":message.get("duration_us").and_then(serde_json::Value::as_u64),
                "next_tick":message.get("next_tick").and_then(serde_json::Value::as_str).filter(|value|matches!(*value,"idle"|"next-frame"|"after-ms")),
                "error_type":message.get("error_type").and_then(serde_json::Value::as_str).filter(|value|matches!(*value,"hard_timeout"|"compile"|"instantiate"|"init"|"render"|"tree_apply_failed"|"canvas_unavailable")),
            }),
        );
        return true;
    }
    let reply = serde_json::json!({
        "type":"control",
        "request":"display_only",
        "response":crate::bridge::action::Response {
            ok:false,
            readonly:true,
            error:Some("このURLは表示専用です。編集には /overlay をOBS Browser SourceのInteractionで開いてください。".into()),
            canvases:Vec::new(),
            generation:None,
            dirty:false,
        }
    });
    let Ok(reply) = serde_json::to_string(&reply) else {
        return false;
    };
    socket.send(Message::Text(reply.into())).await.is_ok()
}

pub(crate) async fn socket(
    Path(id): Path<String>,
    RawQuery(query): RawQuery,
    ws: WebSocketUpgrade,
    State(shared): State<Arc<Shared>>,
) -> Response {
    if !shared
        .canvases
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .any(|canvas| canvas.id == id)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    let sample = query
        .as_deref()
        .is_some_and(|query| query.split('&').any(|part| part == "sample=1"));
    ws.on_upgrade(move |mut socket| async move {
        let mut sent_state = None;
        let mut sent_presentation = None;
        loop {
            let canvas = shared
                .canvases
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|canvas| canvas.id == id)
                .cloned();
            let Some(canvas) = canvas else {
                let message = serde_json::json!({"type":"canvas_unavailable"}).to_string();
                let _ = send_text(&mut socket, message).await;
                break;
            };
            let notified = shared.changed.notified();
            let mut state = shared
                .feed
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            state = display_state(state, sample);
            if sent_state.as_ref() != Some(&state) {
                let Ok(bytes) =
                    serde_json::to_string(&serde_json::json!({"type":"state", "state":state}))
                else {
                    break;
                };
                if !send_text(&mut socket, bytes).await {
                    break;
                }
                sent_state = Some(state);
            }
            let Ok(package) = shared.skins.open(canvas.skin.name()) else {
                break;
            };
            let presentation = canvas_specification(&canvas, &package);
            if sent_presentation.as_ref() != Some(&presentation) {
                let bytes =
                    serde_json::json!({"type":"presentation", "specification":presentation})
                        .to_string();
                if !send_text(&mut socket, bytes).await {
                    break;
                }
                sent_presentation = Some(presentation);
            }
            tokio::select! {
                () = notified => {},
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if shared.feed.stop.load(Ordering::Acquire) { break; }
                },
                message = socket.recv() => {
                    if !handle_canvas_socket_message(&mut socket, message, &id).await {
                        break;
                    }
                }
            }
        }
    })
}

pub(crate) async fn stage_socket(
    ws: WebSocketUpgrade,
    RawQuery(version): RawQuery,
    State(shared): State<Arc<Shared>>,
) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let session = EditorConnection::new(Arc::clone(&shared));
        if version.as_deref() != Some(format!("asset_version={ASSET_VERSION}").as_str()) {
            crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"error", "error_type":"version_mismatch"}));
            let _ = socket.send(Message::Text(version_mismatch().into())).await;
            return;
        }
        crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"success"}));
        let mut sent = String::new();
        loop {
            let notified = shared.changed.notified();
            let stage = stage_snapshot(&shared);
            let message =
                serde_json::json!({"type":"stage", "asset_version":ASSET_VERSION, "state":stage.state, "canvases":stage.canvases})
                    .to_string();
            if message != sent {
                if socket
                    .send(Message::Text(message.clone().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                sent = message;
            }
            tokio::select! {
                () = notified => {},
                () = tokio::time::sleep(Duration::from_millis(250)) => {
                    if shared.feed.stop.load(Ordering::Acquire) { break; }
                },
                message = socket.recv() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            let envelope = serde_json::from_str::<StageControlEnvelope>(&text);
                            let request_id = envelope.as_ref().map_or(0, |value| value.request_id);
                            if envelope.as_ref().is_ok_and(|value| value.asset_version != ASSET_VERSION) {
                                crate::diagnostics::emit("overlay_editor_version", &serde_json::json!({"status":"error", "error_type":"version_mismatch"}));
                                let _ = socket.send(Message::Text(version_mismatch().into())).await;
                                break;
                            }
                            let response = envelope
                                .map_err(|error| error.to_string())
                                .and_then(|envelope| serde_json::from_value(envelope.request).map_err(|error| error.to_string()))
                                .and_then(|request| session.request(request))
                                .unwrap_or_else(|error| crate::bridge::action::Response {
                                    ok:false, readonly:true, error:Some(error),
                                    canvases:Vec::new(), generation:None, dirty:false,
                                });
                            let reply = serde_json::json!({"type":"control", "request_id":request_id, "response":response}).to_string();
                            if socket.send(Message::Text(reply.into())).await.is_err() { break; }
                        }
                        None | Some(Err(_) | Ok(Message::Close(_))) => break,
                        _ => {}
                    }
                },
            }
        }
    })
}
