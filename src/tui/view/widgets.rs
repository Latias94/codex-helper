use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Buffer, Line, Span, Style};
use ratatui::widgets::Widget;

use crate::tui::model::Palette;

pub(super) fn kv_line<'a>(p: Palette, k: &'a str, v: String, v_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{k}: "), Style::default().fg(p.muted)),
        Span::styled(v, v_style),
    ])
}

pub(super) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_background(area: Rect, buf: &mut Buffer, p: Palette) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_style(Style::default().bg(p.bg));
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct BackgroundWidget {
    pub(super) p: Palette,
}

impl Widget for BackgroundWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        render_background(area, buf, self.p);
    }
}
