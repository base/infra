use std::io::Stdout;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use tokio::sync::oneshot;

use super::{Action, Resources, Router, View, ViewId, resources::LoadTestSetup, runner};
use crate::{
    commands::common::EVENT_POLL_TIMEOUT,
    tui::{AppFrame, restore_terminal, setup_terminal},
};

#[derive(Debug)]
pub struct App {
    router: Router,
    resources: Resources,
    show_help: bool,
}

impl App {
    pub const fn new(resources: Resources, initial_view: ViewId) -> Self {
        Self { router: Router::new(initial_view), resources, show_help: false }
    }

    pub async fn run<F>(mut self, mut view_factory: F) -> Result<()>
    where
        F: FnMut(ViewId) -> Box<dyn View>,
    {
        let mut terminal = setup_terminal()?;
        let result = self.run_loop(&mut terminal, &mut view_factory).await;
        restore_terminal(&mut terminal)?;
        result
    }

    async fn run_loop<F>(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
        view_factory: &mut F,
    ) -> Result<()>
    where
        F: FnMut(ViewId) -> Box<dyn View>,
    {
        let mut current_view = view_factory(self.router.current());

        loop {
            self.resources.da.poll();
            self.resources.flash.poll();
            self.resources.poll_sys_config();
            if let Some(ref mut lt) = self.resources.loadtest {
                lt.poll();
            }

            self.poll_loadtest_setup();

            let action = current_view.tick(&mut self.resources);
            if self.handle_action(action, &mut current_view, view_factory) {
                break;
            }

            terminal.draw(|frame| {
                let layout = AppFrame::split_layout(frame.area(), self.show_help);
                current_view.render(frame, layout.content, &self.resources);
                AppFrame::render(
                    frame,
                    &layout,
                    self.resources.chain_name(),
                    current_view.keybindings(),
                );
            })?;

            if event::poll(EVENT_POLL_TIMEOUT)?
                && let Event::Key(key) = event::read()?
            {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }

                let action = match key.code {
                    KeyCode::Char('?') => {
                        self.show_help = !self.show_help;
                        Action::None
                    }
                    KeyCode::Char('q') => Action::Quit,
                    KeyCode::Esc => {
                        if self.router.current() == ViewId::Home {
                            Action::Quit
                        } else {
                            Action::SwitchView(ViewId::Home)
                        }
                    }
                    _ => current_view.handle_key(key, &mut self.resources),
                };

                if self.handle_action(action, &mut current_view, view_factory) {
                    break;
                }
            }
        }

        Ok(())
    }

    fn poll_loadtest_setup(&mut self) {
        match self.resources.loadtest_setup.take() {
            Some(LoadTestSetup::Starting { config_path, mut result_rx }) => {
                match result_rx.try_recv() {
                    Ok(Ok(handle)) => {
                        let config_file = config_path.display().to_string();
                        runner::activate_loadtest(&mut self.resources, handle, config_file);
                    }
                    Ok(Err(e)) => {
                        self.resources.loadtest_setup =
                            Some(LoadTestSetup::Failed { config_path, error: format!("{e:#}") });
                    }
                    Err(oneshot::error::TryRecvError::Empty) => {
                        self.resources.loadtest_setup =
                            Some(LoadTestSetup::Starting { config_path, result_rx });
                    }
                    Err(oneshot::error::TryRecvError::Closed) => {
                        self.resources.loadtest_setup = Some(LoadTestSetup::Failed {
                            config_path,
                            error: "Setup task panicked".to_string(),
                        });
                    }
                }
            }
            other => self.resources.loadtest_setup = other,
        }
    }

    fn handle_action<F>(
        &mut self,
        action: Action,
        current_view: &mut Box<dyn View>,
        view_factory: &mut F,
    ) -> bool
    where
        F: FnMut(ViewId) -> Box<dyn View>,
    {
        match action {
            Action::None => false,
            Action::Quit => true,
            Action::SwitchView(view_id) => {
                self.router.switch_to(view_id);
                *current_view = view_factory(view_id);
                self.show_help = false;
                false
            }
            Action::StartLoadTest(path) => {
                runner::spawn_loadtest_setup(&mut self.resources, path);
                false
            }
        }
    }
}
