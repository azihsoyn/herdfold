//! `herdfold reader attach`: a client of a reader's socket that draws the
//! right-hand page. It checks the protocol with `ping`, attaches with its
//! size, subscribes to `page.shown` and `reader.closed`, and passes every
//! key back through `reader.send_keys`, so the book turns from either pane.

use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event, MouseButton, MouseEventKind};

use crate::api::{
    Call, EventData, EventsSubscribeParams, Incoming, MouseKind, ReaderMouseParams,
    ReaderSendKeysParams, ReaderSizeParams, Subscription,
};
use crate::cli::CliError;
use crate::client::Client;
use crate::turn::Turning;
use crate::view::{self, PageView, Side};

pub fn run() -> Result<(), CliError> {
    let mut client = Client::connect("attach")?;
    let (cols, rows) = crossterm::terminal::size().map_err(CliError::io)?;
    client.call(Call::ReaderAttach(ReaderSizeParams { cols, rows }))?;
    client.call(Call::EventsSubscribe(EventsSubscribeParams {
        subscriptions: vec![Subscription::PageShown, Subscription::ReaderClosed],
    }))?;

    let mut terminal = ratatui::init();
    // Mouse presses go to the reader, which chooses text on this page.
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
    let result = draw(&mut terminal, &mut client);
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    ratatui::restore();
    result
}

fn draw(terminal: &mut ratatui::DefaultTerminal, client: &mut Client) -> Result<(), CliError> {
    let mut page: Option<PageView> = None;
    let mut turning: Option<Turning> = None;
    // What was last drawn, so a quiet page is not written again (which would
    // clear a selection made with the mouse).
    let mut drawn: Option<(Option<PageView>, ratatui::layout::Size)> = None;
    let mut pictures = crate::pictures::Pictures::new();
    loop {
        let size = terminal.size().map_err(CliError::io)?;
        let frame = (page.clone(), size);
        if turning.is_some() || drawn.as_ref() != Some(&frame) {
            view::draw_whole(terminal, |f| {
                let area = f.area();
                let turned = turning
                    .as_ref()
                    .is_some_and(|t| t.render(f.buffer_mut(), area, page.as_ref()));
                if !turned {
                    view::render(f.buffer_mut(), area, page.as_ref());
                }
            })
            .map_err(CliError::io)?;
            drawn = Some(frame);
        }
        if turning.as_ref().is_some_and(Turning::done) {
            turning = None;
            drawn = None;
        }
        let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
        pictures.show(
            area,
            if turning.is_none() {
                page.as_ref()
            } else {
                None
            },
        );
        let wait = if turning.is_some() { 16 } else { 30 };
        if event::poll(Duration::from_millis(wait)).map_err(CliError::io)? {
            match event::read().map_err(CliError::io)? {
                Event::Key(k) => {
                    if let Some(key) = view::key_name(k) {
                        client.send(Call::ReaderSendKeys(ReaderSendKeysParams {
                            keys: vec![key],
                        }))?;
                    }
                }
                Event::Mouse(m) => {
                    let kind = match m.kind {
                        MouseEventKind::Down(MouseButton::Left) => Some(MouseKind::Down),
                        MouseEventKind::Drag(MouseButton::Left) => Some(MouseKind::Drag),
                        MouseEventKind::Up(MouseButton::Left) => Some(MouseKind::Up),
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        client.send(Call::ReaderSendMouse(ReaderMouseParams {
                            kind,
                            col: m.column,
                            row: m.row,
                        }))?;
                    }
                }
                Event::Resize(cols, rows) => {
                    client.send(Call::ReaderResize(ReaderSizeParams { cols, rows }))?;
                }
                _ => {}
            }
        }
        loop {
            match client.incoming.try_recv() {
                Ok(Incoming::Event(e)) => match e.data {
                    EventData::PageShown { right, turn, .. } => {
                        // This pane always holds the right-hand page.
                        turning = turn.map(|t| Turning::new(t, page.take(), Side::Right));
                        page = right;
                    }
                    EventData::ReaderClosed => return Ok(()),
                    // Subscribed to the pages only.
                    _ => {}
                },
                Ok(_) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
            }
        }
    }
}
