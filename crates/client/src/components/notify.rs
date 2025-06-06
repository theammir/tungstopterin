use std::time;

use color_eyre::eyre::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Stylize,
    text::Line,
    widgets::{Block, Clear, Paragraph, Widget},
};

use crate::{AppEvent, component::Component};

#[derive(Debug)]
struct TimedNotification {
    text: String,
    timestamp: time::Instant,
    duration: time::Duration,
}

#[derive(Debug)]
pub struct Notification {
    notifications: Vec<TimedNotification>,
}

impl Notification {
    pub fn new() -> Box<Self> {
        Box::new(Self {
            notifications: vec![],
        })
    }

    fn purge_expired(&mut self) {
        let now = time::Instant::now();
        self.notifications
            .retain(|notif| notif.timestamp + notif.duration >= now)
    }

    fn get_toast_area(&self, paragraph: &Paragraph, area: Rect, y_offset: u16) -> (Rect, u16) {
        let inner_width =
            (area.width.saturating_sub(2)).min(paragraph.line_width().saturating_sub(2) as u16);
        let [_, inner_area_h] =
            Layout::horizontal([Constraint::Fill(1), Constraint::Length(inner_width)]).areas(area);

        // FIX: It doesn't work properly with manual linebreaks.
        // And, according to ratatui #293, with a lot of other things.
        let height = paragraph.line_count(inner_width) as u16;

        let [_, toast_area] =
            Layout::vertical([Constraint::Length(y_offset), Constraint::Length(height)])
                .areas(inner_area_h);

        (toast_area, height)
    }
}

#[async_trait::async_trait]
impl Component for Notification {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _is_focused: bool) {
        self.purge_expired();

        let layout = Layout::horizontal([Constraint::Fill(1), Constraint::Ratio(1, 3)]);
        let [_, notification_area] = layout.areas(area);
        let notification_block = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .bold();

        let mut offset_height: u16 = 0;
        for notif in &self.notifications {
            let paragraph = Paragraph::new(notif.text.lines().map(Line::from).collect::<Vec<_>>())
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(notification_block.clone());
            let (toast_area, height) =
                self.get_toast_area(&paragraph, notification_area, offset_height);

            frame.render_widget(Clear, toast_area);
            paragraph.render(toast_area, frame.buffer_mut());

            offset_height = offset_height.saturating_add(height);
        }
    }

    async fn handle_event(&mut self, event: AppEvent, _is_focused: bool) -> Result<bool> {
        Ok(match event {
            AppEvent::Notify(text, duration) => {
                self.notifications.push(TimedNotification {
                    text,
                    timestamp: time::Instant::now(),
                    duration,
                });
                true
            }
            _ => false,
        })
    }
}
