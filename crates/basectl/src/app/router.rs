#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewId {
    Home,
    CommandCenter,
    DaMonitor,
    Flashblocks,
    Config,
    LoadTest,
}

#[derive(Debug)]
pub struct Router {
    current: ViewId,
}

impl Router {
    pub const fn new(initial: ViewId) -> Self {
        Self { current: initial }
    }

    pub const fn current(&self) -> ViewId {
        self.current
    }

    pub const fn switch_to(&mut self, view: ViewId) {
        self.current = view;
    }
}
