mod command_center;
mod config;
mod da_monitor;
mod factory;
mod flashblocks;
mod flashtest;
mod home;
mod loadtest;

pub use command_center::CommandCenterView;
pub use config::ConfigView;
pub use da_monitor::DaMonitorView;
pub use factory::create_view;
pub use flashblocks::FlashblocksView;
pub use flashtest::FlashTestView;
pub use home::HomeView;
pub use loadtest::LoadTestView;
