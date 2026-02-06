use super::ViewId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    SwitchView(ViewId),
    Back,
    ToggleHelp,
    SetFocus(usize),
}
