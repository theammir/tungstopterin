use color_eyre::eyre::Result;
use common::protocol;
use ratatui::{
    Frame,
    crossterm::event::{self},
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{
        Block, BorderType, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
    },
};
use tokio::sync::mpsc::UnboundedSender;
use tui_input::backend::crossterm::EventHandler;
use websocket::message::Message;

use crate::{AppEvent, component::Component, into_protocol_color};

#[derive(Debug)]
struct ColorList {
    items: Vec<&'static str>,
    state: ListState,
}

impl Default for ColorList {
    fn default() -> Self {
        Self {
            items: vec![
                "red", "yellow", "green", "cyan", "blue", "magenta", "default",
            ],
            state: ListState::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Focus {
    #[default]
    Input,
    Colors,
}

impl Focus {
    fn next(self) -> Self {
        match self {
            Self::Input => Self::Colors,
            Self::Colors => Self::Input,
        }
    }
}

#[derive(Debug)]
pub struct Auth {
    ws_tx: UnboundedSender<Message>,
    event_tx: UnboundedSender<AppEvent>,

    focus: Focus,

    nickname_input: tui_input::Input,
    color_list: ColorList,
}

impl Auth {
    pub fn new(ws_tx: UnboundedSender<Message>, event_tx: UnboundedSender<AppEvent>) -> Box<Self> {
        Box::new(Self {
            ws_tx,
            event_tx,
            focus: Default::default(),
            nickname_input: Default::default(),
            color_list: Default::default(),
        })
    }

    async fn try_authenticate(&mut self) -> Result<()> {
        let selected = self.color_list.state.selected().unwrap();
        self.ws_tx.send(
            protocol::ClientMessage::Auth(protocol::MessageSender {
                name: self.nickname_input.to_string(),
                color: into_protocol_color(
                    self.color_list.items[selected].parse::<Color>().unwrap(),
                ),
            })
            .into(),
        )?;
        Ok(())
    }

    async fn handle_input_event(&mut self, event: event::KeyEvent) -> Result<bool> {
        Ok(self
            .nickname_input
            .handle_event(&event::Event::Key(event))
            .is_some())
    }

    async fn handle_colors_event(&mut self, event: event::KeyEvent) -> Result<bool> {
        Ok(match event.code {
            event::KeyCode::Char('j' | 's') | event::KeyCode::Down => {
                self.color_list.state.select_next();
                true
            }
            event::KeyCode::Char('k' | 'w') | event::KeyCode::Up => {
                self.color_list.state.select_previous();
                true
            }
            _ => false,
        })
    }
}

fn center_area(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

#[async_trait::async_trait]
impl Component for Auth {
    async fn init(&mut self) -> Result<()> {
        self.color_list.state.select_first();
        Ok(())
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool) {
        if !is_focused {
            return;
        }
        let area = center_area(area, Constraint::Ratio(1, 3), Constraint::Ratio(2, 3));
        frame.render_widget(Clear, area);
        let bordered = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().magenta());
        bordered.clone().render(area, frame.buffer_mut());

        let [input_area, color_area] = Layout::vertical([Constraint::Max(3), Constraint::Fill(1)])
            .margin(1)
            .areas(area);
        Paragraph::new(self.nickname_input.value())
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(Block::bordered().border_type(BorderType::Rounded).style(
                if self.focus == Focus::Input {
                    Style::new().magenta()
                } else {
                    Style::new()
                },
            ))
            .render(input_area, frame.buffer_mut());

        let color_items = self.color_list.items.iter().map(|&item| {
            let color = item
                .parse::<ratatui::style::Color>()
                .unwrap_or(Color::Reset);
            ListItem::from(Line::styled(String::from("◼ ") + item, color))
        });

        let color_list = List::new(color_items)
            .block(Block::bordered().border_type(BorderType::Rounded))
            .style(if self.focus == Focus::Colors {
                Style::new().magenta()
            } else {
                Style::new()
            })
            .highlight_symbol(">");
        StatefulWidget::render(
            color_list,
            color_area,
            frame.buffer_mut(),
            &mut self.color_list.state,
        );
    }

    async fn handle_event(&mut self, event: AppEvent, is_focused: bool) -> Result<bool> {
        if !is_focused {
            return Ok(false);
        }
        if let AppEvent::KeyEvent(key_event) = event {
            if match self.focus {
                Focus::Input => self.handle_input_event(key_event).await?,
                Focus::Colors => self.handle_colors_event(key_event).await?,
            } {
                return Ok(true);
            }
            Ok(match key_event.code {
                event::KeyCode::Char('q') => {
                    self.event_tx.send(AppEvent::CompUnfocus)?;
                    true
                }
                event::KeyCode::Tab => {
                    self.focus = self.focus.next();
                    true
                }
                event::KeyCode::Enter => {
                    self.try_authenticate().await?;
                    self.event_tx.send(AppEvent::CompUnfocus)?;
                    true
                }
                _ => false,
            })
        } else {
            Ok(false)
        }
    }
}
