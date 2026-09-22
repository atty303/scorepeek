use scorepeek_frontend_api::RunSnapshot;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

#[must_use]
pub fn plain_status_line(run: &RunSnapshot) -> String {
    format!(
        "scorepeek: state={} sessions={} session={} generation={} channel={} clients={} dropped={} disconnected={} message={} {}{}",
        run.watcher_state,
        run.session_count,
        run.active_session_id.as_deref().unwrap_or("-"),
        run.capture_generation
            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        run.event_stream_status,
        run.connected_clients,
        run.dropped_events,
        run.disconnected_clients,
        run.message,
        run.scores_summary.as_deref().unwrap_or("scores=disabled"),
        run.overlay_summary,
    )
}

pub fn render(frame: &mut ratatui::Frame<'_>, run: &RunSnapshot) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(0),
        ])
        .split(frame.area());

    frame.render_widget(
        Paragraph::new(watcher_lines(run)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(watcher_color(run)))
                .title(run.scores_summary.as_ref().map_or_else(
                    || "Watcher".to_owned(),
                    |scores| format!("Watcher {scores}"),
                )),
        ),
        rows[0],
    );
    frame.render_widget(
        Paragraph::new(result_lines(run)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(result_color(run)))
                .title("Latest result"),
        ),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(music_select_lines(run)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(music_select_color(run)))
                .title("Music Select Resolver"),
        ),
        rows[2],
    );
    frame.render_widget(
        Paragraph::new(resolver_lines(run))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(resolver_color(run)))
                    .title("Resolver"),
            ),
        rows[3],
    );
}

fn watcher_lines(run: &RunSnapshot) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(
                run.watcher_state.clone(),
                Style::default().fg(watcher_color(run)),
            ),
            Span::raw(format!(
                "  raw={} semantic={}  sessions={} gen={}",
                run.raw_screen.as_deref().unwrap_or("-"),
                run.semantic_screen.as_deref().unwrap_or("-"),
                run.session_count,
                run.capture_generation
                    .map_or_else(|| "-".to_owned(), |value| value.to_string()),
            )),
        ]),
        Line::from(format!(
            "recording={} mem={}/{} high={} frame_drop={}  channel={} clients={} drop={} disconnected={}  {}",
            run.recording_status,
            human_bytes(run.recording_memory_used_bytes),
            human_bytes(run.recording_memory_limit_bytes),
            human_bytes(run.recording_memory_high_water_bytes),
            run.recording_dropped_frames,
            run.event_stream_status,
            run.connected_clients,
            run.dropped_events,
            run.disconnected_clients,
            run.message,
        )),
    ]
}

fn result_lines(run: &RunSnapshot) -> Vec<Line<'static>> {
    let label = run.latest_result_label.clone().unwrap_or_else(|| {
        if run.result_count > 0 {
            format!("CONFIRMED #{}", run.result_count)
        } else {
            "-".to_owned()
        }
    });
    if label == "INACTIVE" {
        return vec![Line::from(Span::styled(
            "No active result",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    if run.result_count == 0 && run.latest_result_label.is_none() {
        return vec![Line::from(Span::styled(
            "No result payload yet",
            Style::default().fg(Color::DarkGray),
        ))];
    }
    vec![
        Line::from(vec![
            Span::styled(
                label,
                Style::default()
                    .fg(result_color(run))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  confirmed={}", run.result_count)),
        ]),
        Line::from(format!(
            "session={} generation={}",
            run.active_session_id.as_deref().unwrap_or("-"),
            run.capture_generation
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        )),
    ]
}

fn music_select_lines(run: &RunSnapshot) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "SCREEN raw={} semantic={}",
            run.raw_screen.as_deref().unwrap_or("-"),
            run.semantic_screen.as_deref().unwrap_or("-"),
        )),
        Line::from(format!(
            "session={} generation={}",
            run.active_session_id.as_deref().unwrap_or("-"),
            run.capture_generation
                .map_or_else(|| "-".to_owned(), |value| value.to_string()),
        )),
        Line::from(format!(
            "latest={} results={}",
            run.latest_result_label.as_deref().unwrap_or("-"),
            run.result_count,
        )),
    ]
}

fn resolver_lines(run: &RunSnapshot) -> Vec<Line<'static>> {
    vec![
        Line::from(format!(
            "SCREEN raw={} semantic={}",
            run.raw_screen.as_deref().unwrap_or("-"),
            run.semantic_screen.as_deref().unwrap_or("-"),
        )),
        Line::from(format!(
            "EVENT channel={} clients={} drop={} disconnected={}",
            run.event_stream_status,
            run.connected_clients,
            run.dropped_events,
            run.disconnected_clients,
        )),
        Line::from(format!("STATUS {}", run.message)),
        Line::from(format!("OVERLAY {}", run.overlay_summary)),
    ]
}

fn watcher_color(run: &RunSnapshot) -> Color {
    if run.event_stream_status == "degraded"
        || run.dropped_events > 0
        || run.recording_status == "degraded"
    {
        Color::Red
    } else if run.watcher_state == "stopped" {
        Color::DarkGray
    } else if run.watcher_state == "starting" {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn result_color(run: &RunSnapshot) -> Color {
    match run.latest_result_label.as_deref() {
        Some("RETRACTED") => Color::Red,
        Some("PROVISIONAL") => Color::Yellow,
        None if run.result_count > 0 => Color::Green,
        Some("INACTIVE") | None => Color::DarkGray,
        Some(_) => Color::Green,
    }
}

fn music_select_color(run: &RunSnapshot) -> Color {
    if run.semantic_screen.as_deref() == Some("music_select") {
        Color::Cyan
    } else {
        Color::DarkGray
    }
}

fn resolver_color(run: &RunSnapshot) -> Color {
    if run.semantic_screen.as_deref() == Some("result") {
        result_color(run)
    } else {
        Color::DarkGray
    }
}

fn human_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if bytes >= MIB {
        format!(
            "{}.{}MiB",
            bytes / MIB,
            (bytes % MIB).saturating_mul(10) / MIB
        )
    } else if bytes >= KIB {
        format!(
            "{}.{}KiB",
            bytes / KIB,
            (bytes % KIB).saturating_mul(10) / KIB
        )
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn run_snapshot() -> RunSnapshot {
        RunSnapshot {
            watcher_state: "session_active".to_owned(),
            session_count: 2,
            active_session_id: Some("session-2".to_owned()),
            capture_generation: Some(3),
            raw_screen: Some("result".to_owned()),
            semantic_screen: Some("result".to_owned()),
            recording_status: "active".to_owned(),
            recording_memory_used_bytes: 1024,
            recording_memory_limit_bytes: 2048,
            event_stream_status: "active".to_owned(),
            connected_clients: 1,
            result_count: 4,
            latest_result_label: Some("CONFIRMED".to_owned()),
            message: "running".to_owned(),
            ..RunSnapshot::default()
        }
    }

    #[test]
    fn interactive_view_keeps_the_four_bordered_panels() {
        let mut terminal = Terminal::new(TestBackend::new(80, 25)).unwrap();
        terminal
            .draw(|frame| render(frame, &run_snapshot()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        for title in [
            "Watcher",
            "Latest result",
            "Music Select Resolver",
            "Resolver",
        ] {
            assert!(text.contains(title), "missing panel title {title}");
        }
        for (top, bottom) in [(0, 3), (4, 11), (12, 18), (19, 24)] {
            assert_eq!(buffer[(0, top)].symbol(), "┌");
            assert_eq!(buffer[(79, top)].symbol(), "┐");
            assert_eq!(buffer[(0, bottom)].symbol(), "└");
            assert_eq!(buffer[(79, bottom)].symbol(), "┘");
        }
        assert!(text.contains("CONFIRMED"));
    }

    #[test]
    fn plain_status_keeps_the_existing_single_line_contract() {
        let line = plain_status_line(&run_snapshot());
        assert!(line.starts_with("scorepeek: state=session_active sessions=2"));
        assert!(line.contains("channel=active clients=1"));
        assert!(!line.contains('\n'));
    }
}
