/// An effect for the runtime to carry out on the model's behalf.
///
/// [`Model::update`](crate::tui::Model::update) returns these instead of
/// performing the work itself, so it stays synchronous and free of any
/// dependency on the async runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Command {
    /// Leave the main loop.
    ///
    /// Quitting is an effect rather than model state: nothing about the
    /// catalogue or the interface changes, the loop simply stops.
    Quit,
}
