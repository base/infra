#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewId {
    Home,
    CommandCenter,
    DaMonitor,
    Flashblocks,
    Config,
}

pub struct Router {
    current: ViewId,
    history: Vec<ViewId>,
}

impl Router {
    pub fn new(initial: ViewId) -> Self {
        Self { current: initial, history: Vec::new() }
    }

    pub fn current(&self) -> ViewId {
        self.current
    }

    pub fn switch_to(&mut self, view: ViewId) {
        if view != self.current {
            self.history.push(self.current);
            self.current = view;
        }
    }

    pub fn back(&mut self) -> bool {
        if let Some(prev) = self.history.pop() {
            self.current = prev;
            true
        } else {
            false
        }
    }

    pub fn go_home(&mut self) {
        self.history.clear();
        self.current = ViewId::Home;
    }
}
