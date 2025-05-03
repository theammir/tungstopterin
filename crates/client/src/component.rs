use color_eyre::eyre::Result;
use ratatui::{Frame, layout::Rect};

use crate::AppEvent;

#[async_trait::async_trait]
pub trait Component: std::fmt::Debug {
    async fn init(&mut self) -> Result<()> {
        Ok(())
    }
    fn render(&mut self, frame: &mut Frame, area: Rect, is_focused: bool);
    async fn handle_event(&mut self, event: AppEvent, is_focused: bool) -> Result<bool>;
}
