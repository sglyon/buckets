# Buckets terminal

A GPUI terminal view backed by Alacritty's ANSI model and a real portable-pty pseudoterminal. The workspace owns each session; plugins return an executable, literal argv, working directory and environment. There are no Neovim or Lazygit branches in this crate.

`TerminalSession::spawn(TerminalSpec)` can run on a background executor. Construct `TerminalView::new(session, window, cx)` on the UI thread. The view implements `Render` and `Focusable`, measures its grid from actual layout and resizes both Alacritty and the PTY. Session output remains visible after process exit. Dropping or closing the view terminates its owned PTY session jobs; deliberately detached processes that start a new OS session are outside that ownership.

PTY reads, ANSI parsing, writes and child waiting run off the UI thread. Changes coalesce into one UI invalidation per eight milliseconds; idle sessions do not rebuild grid snapshots. Painting groups adjacent ASCII cells with identical styles, draws only the visible grid and uses GPUI's text shaping cache. Unicode cells use natural shaping to preserve combining marks and wide characters. Scrollback is bounded to 10,000 lines; queued input is bounded to 128 messages with a four MiB limit on each write.

The view supports alternate screens, cursor modes, 256/true color, bold/italic/underline/strikeout, inverse/dim text, OSC title and copy, terminal query replies, bracketed paste, native text input/preedit, left-button terminal mouse reporting, mouse selection and scrolling. Shift overrides application mouse reporting for host selection. Option is Meta (Option-s sends ESC then s); Command shortcuts remain workspace commands. Kitty keyboard protocol is disabled. Graphics protocols, hyperlink opening, terminal search and full accessibility text exposure are not implemented.

Keyboard controls:

- Typing, Tab, Escape, Return, arrows, function keys, Ctrl combinations and Option combinations go to the terminal application.
- Command-A selects the entire current buffer, including scrollback; Command-C copies the selection. Control-Shift-C also copies.
- Command-V or Control-Shift-V pastes. Embedded Escape characters are removed from paste so they cannot terminate a bracketed-paste envelope.
- Shift-PageUp/Down and Shift-Home/End navigate scrollback outside an alternate screen. Full-screen applications receive these keys while using an alternate screen.
- The workspace reserves Command-Option-Left/Right to move focus between panes and Command-W to close the active pane.

Input and resize failures appear in a rose status strip inside the terminal. Successful input clears the error. `last_error`, `exited` and `exit_status` also expose state to the host.
