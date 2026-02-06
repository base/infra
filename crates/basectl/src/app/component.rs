use crossterm::event::KeyEvent;
use ratatui::{Frame, layout::Rect};

use super::{Action, Resources};

pub trait Component {
    fn handle_key(&mut self, key: KeyEvent, resources: &mut Resources) -> Action;

    fn tick(&mut self, resources: &mut Resources) -> Action {
        let _ = resources;
        Action::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, resources: &Resources, focused: bool);
}
