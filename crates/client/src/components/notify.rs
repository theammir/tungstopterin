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
            let inner_width = (notification_area.width - 2)
                .min(notif.text.lines().map(|line| line.len()).max().unwrap_or(0) as u16);
            let [_, inner_area_h] =
                Layout::horizontal([Constraint::Fill(1), Constraint::Length(inner_width)])
                    .areas(notification_area);
            let paragraph = Paragraph::new(notif.text.lines().map(Line::from).collect::<Vec<_>>())
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(notification_block.clone());

            // FIX: It doesn't work properly with manual linebreaks.
            // And, according to ratatui #293, with a lot of other things.
            let height = paragraph.line_count(inner_width) as u16;
            let [_, inner_area] = Layout::vertical([
                Constraint::Length(offset_height),
                Constraint::Length(height),
            ])
            .areas(inner_area_h);
            offset_height = offset_height.saturating_add(height);

            frame.render_widget(Clear, inner_area);
            paragraph.render(inner_area, frame.buffer_mut());
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
