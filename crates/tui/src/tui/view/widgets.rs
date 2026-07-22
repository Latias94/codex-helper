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

pub(super) fn max_wrapped_vertical_scroll(
    lines: &[Line<'_>],
    text_width: u16,
    viewport_height: u16,
) -> u16 {
    if text_width == 0 || viewport_height == 0 {
        return 0;
    }

    wrapped_visual_line_count(lines, usize::from(text_width))
        .saturating_sub(usize::from(viewport_height))
        .min(usize::from(u16::MAX)) as u16
}

pub(super) fn master_detail_fits(
    area: Rect,
    master_percent: u16,
    master_min_width: u16,
    detail_min_width: u16,
) -> bool {
    let master_width = area.width.saturating_mul(master_percent) / 100;
    let detail_width = area.width.saturating_sub(master_width);
    master_width >= master_min_width && detail_width >= detail_min_width
}

fn wrapped_visual_line_count(lines: &[Line<'_>], text_width: usize) -> usize {
    if text_width == 0 {
        return 0;
    }

    lines
        .iter()
        .map(|line| {
            let width = line.width();
            if width == 0 {
                1
            } else {
                width.div_ceil(text_width)
            }
        })
        .sum()
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
            let cell = &mut buf[(x, y)];
            cell.set_symbol(" ");
            cell.set_style(Style::default().bg(p.bg));
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

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui::prelude::{Buffer, Line};

    use super::{max_wrapped_vertical_scroll, render_background};
    use crate::tui::model::Palette;

    #[test]
    fn background_clears_symbols() {
        let p = Palette::default();
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_symbol("x");
            }
        }

        render_background(area, &mut buf, p);

        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                assert_eq!(buf[(x, y)].symbol(), " ");
            }
        }
    }

    #[test]
    fn wrapped_scroll_limit_counts_visual_lines() {
        let lines = vec![Line::from("12345678901234567890")];

        assert_eq!(max_wrapped_vertical_scroll(&lines, 5, 3), 1);
    }
}
