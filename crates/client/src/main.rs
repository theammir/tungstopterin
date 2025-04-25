use std::time::Duration;

use color_eyre::eyre::Result;
use ratatui::{
    DefaultTerminal,
    crossterm::{
        self,
        event::{self, KeyEvent},
    },
    prelude::*,
    widgets::{Block, Paragraph},
};
use tokio::{net::TcpStream, sync::mpsc::UnboundedReceiver};
use tokio_util::sync::CancellationToken;
use tui_input::backend::crossterm::EventHandler;
use websocket::{
    Server, WsRecv, WsSend, WsSendHalf, WsStream, handshake::IntoWebsocket, message::Message,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    WsMessage(Message),
    KeyEvent(crossterm::event::KeyEvent),
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
}

#[derive(Debug)]
struct App {
    should_quit: bool,

    mode: Mode,

    event_rx: UnboundedReceiver<Event>,
    ws_tx: WsSendHalf<Server>,

    current_input: tui_input::Input,
    received_messages: Vec<String>,
}

impl Widget for &App {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let layout = Layout::vertical([Constraint::Fill(1), Constraint::Max(5)]);
        let [chat_area, input_area] = layout.areas(area);
        let bordered = Block::bordered().border_type(ratatui::widgets::BorderType::Rounded);
        Paragraph::new(
            self.received_messages
                .iter()
                .cloned()
                .map(|line| line + "\n")
                .reduce(|acc, i| acc + &i)
                .unwrap_or(String::new())
                .trim(),
        )
        .block(bordered.clone())
        .scroll((
            0,
            self.current_input.visual_scroll(input_area.width as usize) as u16,
        ))
        .wrap(ratatui::widgets::Wrap { trim: true })
        .render(chat_area, buf);
        Paragraph::new(self.current_input.value())
            .block(if self.mode == Mode::Insert {
                bordered.blue()
            } else {
                bordered
            })
            .wrap(ratatui::widgets::Wrap { trim: false })
            .render(input_area, buf);
    }
}

impl App {
    fn new(event_rx: UnboundedReceiver<Event>, ws_tx: WsSendHalf<Server>) -> Self {
        App {
            should_quit: false,
            mode: Mode::Normal,
            event_rx,
            ws_tx,
            current_input: tui_input::Input::new(String::new()),
            received_messages: vec![],
        }
    }

    async fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            self.handle_events().await?;
            terminal.draw(|frame| self.draw(frame))?;
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(self, frame.area());
    }

    async fn handle_events(&mut self) -> Result<()> {
        if let Ok(event) = self.event_rx.try_recv() {
            match event {
                Event::KeyEvent(key_event) => {
                    self.handle_key_event(key_event).await?;
                }
                Event::WsMessage(msg) => {
                    self.handle_ws_message(msg).await?;
                }
            }
        }
        Ok(())
    }

    async fn handle_key_event(&mut self, event: KeyEvent) -> Result<()> {
        match self.mode {
            Mode::Normal => match event.code {
                event::KeyCode::Char('q') => {
                    self.should_quit = true;
                }
                event::KeyCode::Char('i') | event::KeyCode::Char('a') => {
                    self.mode = Mode::Insert;
                }
                _ => {}
            },
            Mode::Insert => match event.code {
                event::KeyCode::Esc => {
                    self.mode = Mode::Normal;
                }
                event::KeyCode::Enter => {
                    self.send_chat_message().await?;
                }
                _ => {
                    self.current_input.handle_event(&event::Event::Key(event));
                }
            },
        }
        Ok(())
    }
    async fn handle_ws_message(&mut self, message: Message) -> Result<()> {
        if let Message::Text(msg) = message {
            self.received_messages.push(msg);
        }
        Ok(())
    }

    async fn send_chat_message(&mut self) -> Result<()> {
        self.ws_tx
            .send(Message::Text(self.current_input.to_string()))
            .await?;
        self.current_input.reset();
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;

    let mut ws = WsStream::from_stream(TcpStream::connect("127.0.0.1:1337").await?);
    ws.try_upgrade().await?;
    let (mut ws_rx, ws_tx) = ws.into_split();

    let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    let fut_tx = event_tx.clone();
    tokio::spawn(async move {
        let event_tx = fut_tx;
        while let Ok(msg) = ws_rx.receive().await {
            _ = event_tx.send(Event::WsMessage(msg));
        }
    });

    // PERF: Dependency on tokio-util
    let event_cancel = CancellationToken::new();
    let event_cancel_inner = event_cancel.child_token();
    let fut_tx = event_tx.clone();
    tokio::spawn(async move {
        let event_tx = fut_tx;
        loop {
            if event_cancel_inner.is_cancelled() {
                break;
            }
            if matches!(crossterm::event::poll(Duration::from_millis(50)), Ok(true)) {
                if let Ok(crossterm::event::Event::Key(event)) = crossterm::event::read() {
                    _ = event_tx.send(Event::KeyEvent(event));
                }
            }
        }
    });

    let mut terminal = ratatui::init();
    let mut app = App::new(event_rx, ws_tx);
    app.run(&mut terminal).await?;

    ratatui::restore();
    event_cancel.cancel();
    Ok(())
}
