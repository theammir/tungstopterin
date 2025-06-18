#![allow(clippy::cast_possible_truncation)]
use std::time;

use color_eyre::eyre::Result;
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Text},
    widgets::{Block, Clear, Paragraph, Widget},
};

use crate::{AppEvent, component::Component};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Urgency {
    Info,
    Warning,
    Error,
}

impl Urgency {
    #[must_use]
    pub fn icon(&self) -> &'static str {
        match self {
            Urgency::Info => "",
            Urgency::Warning => "",
            Urgency::Error => "",
        }
    }

    #[must_use]
    pub fn style(&self) -> Style {
        match self {
            Urgency::Info => Style::new().cyan(),
            Urgency::Warning => Style::new().yellow(),
            Urgency::Error => Style::new().red(),
        }
    }
}

#[derive(Debug)]
struct TimedNotification<'a> {
    text: Text<'a>,
    urgency: Urgency,
    timestamp: time::Instant,
    duration: time::Duration,
}

#[derive(Debug)]
pub struct Notification<'a> {
    notifications: Vec<TimedNotification<'a>>,
}

impl Notification<'_> {
    #[must_use]
    pub fn new() -> Box<Self> {
        Box::new(Self {
            notifications: vec![],
        })
    }

    fn purge_expired(&mut self) {
        let now = time::Instant::now();
        self.notifications
            .retain(|notif| notif.timestamp + notif.duration >= now);
    }

    fn get_toast_area(paragraph: &Paragraph, area: Rect, y_offset: u16) -> (Rect, u16) {
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
impl Component for Notification<'_> {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _is_focused: bool) {
        self.purge_expired();

        let layout = Layout::horizontal([Constraint::Fill(1), Constraint::Ratio(1, 3)]);
        let [_, notification_area] = layout.areas(area);
        let bordered = Block::bordered()
            .border_type(ratatui::widgets::BorderType::Rounded)
            .bold();

        let mut offset_height: u16 = 0;
        for notif in &self.notifications {
            let icon = format!(" {}  ", notif.urgency.icon());
            let paragraph = Paragraph::new(notif.text.clone())
                .wrap(ratatui::widgets::Wrap { trim: false })
                .block(
                    bordered
                        .clone()
                        .padding(ratatui::widgets::Padding::left(1))
                        .style(notif.urgency.style())
                        .title_top(Line::from(icon).centered()),
                );
            let (toast_area, height) =
                Notification::get_toast_area(&paragraph, notification_area, offset_height);

            frame.render_widget(Clear, toast_area);
            paragraph.render(toast_area, frame.buffer_mut());

            offset_height = offset_height.saturating_add(height);
        }
    }

    async fn handle_event(&mut self, event: AppEvent, _is_focused: bool) -> Result<bool> {
        Ok(match event {
            AppEvent::Notify(text, urgency, duration) => {
                self.notifications.push(TimedNotification {
                    text,
                    urgency,
                    timestamp: time::Instant::now(),
                    duration,
                });
                true
            }
            _ => false,
        })
    }
}
