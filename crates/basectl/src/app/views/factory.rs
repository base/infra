use super::{
    CommandCenterView, ConfigView, DaMonitorView, FlashblocksView, HomeView, LoadTestView,
};
use crate::app::{View, ViewId};

pub fn create_view(view_id: ViewId) -> Box<dyn View> {
    match view_id {
        ViewId::Home => Box::new(HomeView::new()),
        ViewId::CommandCenter => Box::new(CommandCenterView::new()),
        ViewId::DaMonitor => Box::new(DaMonitorView::new()),
        ViewId::Flashblocks => Box::new(FlashblocksView::new()),
        ViewId::Config => Box::new(ConfigView::new()),
        ViewId::LoadTest => Box::new(LoadTestView::new()),
    }
}
