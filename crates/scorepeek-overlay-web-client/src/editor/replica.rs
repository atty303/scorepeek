pub use scorepeek_overlay::editor_model::*;
#[cfg(test)]
mod tests {
    use super::*;
    use scorepeek_overlay::{
        AspectRatio,
        editor::{EditorAction, EditorProperty, EditorSkin, EditorTitleState, EditorWidgetDefault},
    };
    fn editor() -> EditorSession {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../scorepeek-overlay/tests/fixtures/visual-composition.json"
        ))
        .unwrap();
        let mut model = EditorSession::new(
            serde_json::from_value(fixture["canvases"].clone()).unwrap(),
            [1920, 1080],
            "obs",
        );
        model.editing = true;
        model.readonly = false;
        model.selected_widget = Some("cam".into());
        model.set_skins(vec![EditorSkin {
            id: model.draft[0].skin,
            name: "fixture".into(),
            release: "1".into(),
            preview: String::new(),
            preview_video: None,
            widget_defaults: [
                ("status", [544, 44]),
                ("selection", [544, 124]),
                ("score", [544, 200]),
                ("history-list", [544, 156]),
                ("history-graph", [544, 208]),
                ("empty", [640, 360]),
            ]
            .into_iter()
            .map(|(kind, [width, height])| (kind.into(), EditorWidgetDefault { width, height }))
            .collect(),
            canvas_properties: std::collections::BTreeMap::from([(
                "background".into(),
                EditorProperty::Enum {
                    default: "none".into(),
                    values: vec!["none".into(), "static".into(), "animated".into()],
                },
            )]),
            widget_properties: std::collections::BTreeMap::new(),
        }]);
        model
    }
    #[test]
    fn backend_workspaces_share_canvas_ids_and_undo_closes_unconfirmed_title() {
        let mut obs = editor();
        let mut native = EditorSession::new(obs.draft.clone(), obs.viewport, "wayland");
        native.readonly = false;
        assert!(obs.action(&EditorAction::AddCanvas));
        assert!(native.action(&EditorAction::AddCanvas));
        assert_eq!(obs.selected_canvas, native.selected_canvas);
        obs.action(&EditorAction::SelectCanvas("design".into()));
        obs.action(&EditorAction::SelectWidget {
            canvas_id: "design".into(),
            widget_id: "cam".into(),
        });
        obs.action(&EditorAction::EditTitle);
        obs.title.as_mut().unwrap().text = "not committed".into();
        assert!(obs.action(&EditorAction::Undo));
        assert!(obs.title.is_none());
        assert_eq!(obs.current().unwrap().id, "design");
        assert_ne!(
            obs.current()
                .unwrap()
                .widgets
                .iter()
                .find(|w| w.id == "cam")
                .unwrap()
                .settings
                .title,
            "not committed"
        );
    }
    #[test]
    fn stage_sync_adds_removes_and_refreshes_canvases_without_overwriting_edits() {
        let mut model = editor();
        let mut remote = model.draft.clone();
        let mut added = remote[0].clone();
        added.id = "another".into();
        remote.push(added);
        model.receive_stage(remote.clone());
        assert_eq!(model.draft.len(), 1);
        model.close();
        model.receive_stage(remote);
        assert_eq!(model.draft.len(), 2);
        let mut remote = model.draft.clone();
        remote.remove(0);
        remote[0]
            .skin_properties
            .insert("background".into(), serde_json::json!("static"));
        model.receive_stage(remote.clone());
        assert_eq!(model.draft, remote);
        assert_eq!(model.saved, remote);
    }
    #[test]
    fn title_composition_survives_reactive_reads_and_only_applies_on_accept() {
        let mut model = editor();
        let original = model.draft.clone();
        model.action(&EditorAction::EditTitle);
        let title = model.title.as_mut().unwrap();
        title.text = "HAND CAM / 上面カメラ".into();
        title.composing = true;
        assert!(!model.action(&EditorAction::AcceptTitle));
        assert_eq!(model.draft, original);
        assert!(model.view().title == EditorTitleState::Composing);
        model.title.as_mut().unwrap().composing = false;
        assert!(model.action(&EditorAction::AcceptTitle));
        assert!(model.title.is_none());
        assert_eq!(
            model.current().unwrap().widgets[1].settings.title,
            "HAND CAM / 上面カメラ"
        );
        assert!(model.action(&EditorAction::Undo));
        assert_eq!(model.draft, original);
    }
    #[test]
    fn denied_edits_and_navigation_preserve_draft_and_undo() {
        let mut model = editor();
        assert!(model.action(&EditorAction::CanvasSkinProperty(
            "background".into(),
            serde_json::json!("none")
        )));
        let changed = model.draft.clone();
        let undo = model.undo.clone();
        model.readonly = true;
        assert!(!model.action(&EditorAction::DeleteWidget));
        assert!(!model.action(&EditorAction::DeleteCanvas));
        assert!(!model.action(&EditorAction::Undo));
        model.action(&EditorAction::TogglePanel);
        model.action(&EditorAction::SelectWidget {
            canvas_id: "design".into(),
            widget_id: "game".into(),
        });
        assert_eq!(model.draft, changed);
        assert_eq!(model.undo, undo);
    }
    #[test]
    fn no_op_drag_preserves_last_undo_and_ratio_resize_stays_in_canvas() {
        let mut model = editor();
        assert!(model.action(&EditorAction::AspectRatio(1)));
        let undo = model.undo.clone();
        model.begin_drag("design".into(), Some("cam".into()), None, [0, 0]);
        model.move_pointer([0, 0]);
        assert!(!model.end_drag());
        assert_eq!(model.undo, undo);
        model.begin_drag(
            "design".into(),
            Some("cam".into()),
            Some("se".into()),
            [0, 0],
        );
        model.move_pointer([2000, 2000]);
        assert!(model.end_drag());
        let canvas = model.current().unwrap();
        let widget = &canvas.widgets[1];
        assert!(widget.width >= 16 && widget.height >= 16);
        assert!(u32::try_from(widget.x).unwrap() + widget.width <= canvas.width);
        assert!(u32::try_from(widget.y).unwrap() + widget.height <= canvas.height);
        assert!((i64::from(widget.width) * 9 - i64::from(widget.height) * 16).abs() <= 64);
    }
    #[test]
    fn all_aspect_modes_and_complete_backend_undo_are_reactive() {
        let mut model = editor();
        let original = model.draft.clone();
        for index in [1, 2, 3, 0] {
            assert!(model.action(&EditorAction::AspectRatio(index)));
            assert_eq!(
                scorepeek_overlay::editor::aspect_ratio_index(
                    model.current().unwrap().widgets[1].settings.aspect_ratio
                ),
                index
            );
        }
        assert!(model.action(&EditorAction::AddCanvas));
        assert_eq!(model.draft.len(), original.len() + 1);
        assert!(model.action(&EditorAction::Undo));
        assert_eq!(model.draft, original);
    }
    #[test]
    fn fixed_ratio_resize_keeps_minimum_dimensions_for_extreme_aspects() {
        for aspect in [
            AspectRatio::Wide,
            AspectRatio::Standard,
            AspectRatio::Current([160, 16]),
            AspectRatio::Current([16, 160]),
        ] {
            let rect = resize(
                [40, 40, 160, 160],
                [-300, -300],
                Some("se"),
                [400, 400],
                [16, 16],
                aspect,
            );
            assert!(rect[2] >= 16 && rect[3] >= 16);
            assert!(rect[0] + rect[2] <= 400 && rect[1] + rect[3] <= 400);
        }
    }
}
