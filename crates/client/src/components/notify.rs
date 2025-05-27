use std::time;

use color_eyre::eyre::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Line,
    widgets::{Paragraph, Widget},
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

        let layout = Layout::horizontal([Constraint::Fill(1), Constraint::Percentage(20)]);
        let [_, notification_area] = layout.areas(area);
        let notification_lines: Vec<Line> = self
            .notifications
            .iter()
            .flat_map(|notif| notif.text.lines())
            .map(|line| Line::styled(line, Style::new().black().on_green().bold()))
            .collect();
        Paragraph::new(notification_lines)
            .right_aligned()
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(notification_area, frame.buffer_mut());
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

