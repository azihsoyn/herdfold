//! The reader's API: every change to the book goes through `Reader::call`,
//! whether it comes over the socket, from the command line or from a key.
//! Each sends the events for what it changed (see `Reader::emit`); panes,
//! agents and the reading log take them from there.

use super::*;
use crate::api::{
    BookmarkAddParams, BookmarkInfo, BookmarkUpdateParams, ChapterInfo, EventData, FindInfo,
    IndexParams, LinkInfo, Move, NoteAddParams, NoteInfo, NoteUpdateParams, QuestionAskParams,
    ReaderGoToParams, ReaderSetParams, ReaderSettings, ReaderStateInfo, TurnDirection,
};

/// A call refused: herdr's error code, and why.
#[derive(Debug, PartialEq)]
pub(super) struct Refused {
    pub code: &'static str,
    pub message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refused {
    Refused {
        code,
        message: message.into(),
    }
}

impl Reader {
    /// Sends `event` to subscribers and the reading log, once the loop
    /// comes round.
    pub(super) fn emit(&mut self, event: EventData) {
        self.outbox.push(event);
    }

    /// Carries out `call`. Calls about the connection itself (attaching,
    /// subscribing, keys and mouse from a pane) are answered by `answer`.
    pub(super) fn call(&mut self, call: Call) -> Result<ResponseResult, Refused> {
        match call {
            Call::Ping(_)
            | Call::EventsSubscribe(_)
            | Call::ReaderAttach(_)
            | Call::ReaderResize(_)
            | Call::ReaderSendKeys(_)
            | Call::ReaderSendMouse(_) => {
                Err(refuse("invalid_request", "only answered over the socket"))
            }
            Call::ReaderState(_) => Ok(ResponseResult::ReaderState {
                state: self.state(),
            }),
            Call::ReaderTurn(p) => {
                let page = self.page();
                match p.direction {
                    TurnDirection::Forward if page + self.step() < self.layout.page_count() => {
                        self.turn_to(page + self.step(), Turn::Forward);
                    }
                    TurnDirection::Backward if page > 0 => {
                        self.turn_to(page.saturating_sub(self.step()), Turn::Backward);
                    }
                    _ => {}
                }
                Ok(self.moved())
            }
            Call::ReaderGoTo(p) => {
                let page = self.page_to(p)?;
                self.jump(page);
                Ok(self.moved())
            }
            Call::ReaderBack(_) => {
                let at = self
                    .trail
                    .pop()
                    .ok_or_else(|| refuse("nowhere_to_go_back", "Nowhere to go back to"))?;
                self.entry.at = at;
                self.moving = Some(Move::Back);
                self.save();
                let page = self.layout.page_of(at);
                self.say(format!("Back to p.{}", page + 1));
                Ok(self.moved())
            }
            Call::ReaderSet(p) => {
                self.set(p)?;
                let settings = self.settings();
                self.emit(EventData::SettingsChanged { settings });
                Ok(ResponseResult::Settings { settings })
            }
            Call::ReaderClose(_) => {
                self.closing = true;
                Ok(ResponseResult::Ok)
            }
            Call::ContentsList(_) => Ok(ResponseResult::Contents {
                chapters: self
                    .doc
                    .chapters
                    .iter()
                    .enumerate()
                    .map(|(index, c)| ChapterInfo {
                        index,
                        title: c.title.clone(),
                        level: c.level,
                        page: self.chapter_pages.get(index).map_or(1, |p| p + 1),
                    })
                    .collect(),
            }),
            Call::SearchRun(p) => {
                let finds = search(&self.doc, &p.query)
                    .into_iter()
                    .map(|(at, end)| FindInfo {
                        at,
                        end,
                        page: self.layout.page_of(at) + 1,
                        context: self.row_text(at),
                    })
                    .collect::<Vec<_>>();
                self.emit(EventData::SearchDone {
                    query: p.query.clone(),
                    finds: finds.len(),
                });
                Ok(ResponseResult::SearchResults {
                    query: p.query,
                    finds,
                })
            }
            Call::LinkList(p) => {
                let open = self.open_links();
                let links = (0..self.doc.links.len())
                    .filter(|i| !p.open || open.contains(i))
                    .map(|i| self.link_info(i))
                    .collect();
                Ok(ResponseResult::Links { links })
            }
            Call::LinkFollow(IndexParams { index }) => {
                let link = *self
                    .doc
                    .links
                    .get(index)
                    .ok_or_else(|| refuse("not_found", format!("no link {index}")))?;
                let page = self.layout.page_of(Pos {
                    line: link.target,
                    offset: 0,
                });
                self.jump(page);
                Ok(self.moved())
            }
            Call::BookmarkList(_) => Ok(ResponseResult::Bookmarks {
                bookmarks: (0..self.entry.marks.len())
                    .map(|i| self.bookmark_info(i))
                    .collect(),
            }),
            Call::BookmarkAdd(p) => self.add_bookmark(p),
            Call::BookmarkUpdate(BookmarkUpdateParams { index, color }) => {
                let mark = self
                    .entry
                    .marks
                    .get_mut(index)
                    .ok_or_else(|| refuse("not_found", format!("no bookmark {index}")))?;
                mark.color = color;
                let at = mark.at;
                // New bookmarks take the colour last chosen.
                self.ribbon = color;
                if let Err(e) = self.update_settings(|s| s.ribbon = Some(color)) {
                    self.say(format!("Could not save the setting: {e}"));
                } else {
                    self.say(format!("Ribbon: {}", color.name()));
                }
                self.save();
                let at = self.place(at);
                self.emit(EventData::BookmarkChanged { at, color });
                Ok(ResponseResult::Ok)
            }
            Call::BookmarkRemove(IndexParams { index }) => {
                if index >= self.entry.marks.len() {
                    return Err(refuse("not_found", format!("no bookmark {index}")));
                }
                let mark = self.entry.marks.remove(index);
                self.say("Bookmark removed".into());
                self.save();
                let at = self.place(mark.at);
                self.emit(EventData::BookmarkRemoved { at });
                Ok(ResponseResult::Ok)
            }
            Call::NoteList(_) => Ok(ResponseResult::Notes {
                notes: (0..self.entry.notes.len())
                    .map(|i| self.note_info(i))
                    .collect(),
            }),
            Call::NoteAdd(p) => self.add_note_call(p),
            Call::NoteUpdate(NoteUpdateParams { index, text, color }) => {
                let note = self
                    .entry
                    .notes
                    .get_mut(index)
                    .ok_or_else(|| refuse("not_found", format!("no note {index}")))?;
                if let Some(text) = text {
                    note.text = text.trim().to_string();
                }
                if let Some(color) = color {
                    note.color = Some(color);
                }
                let (at, text, kept) = (note.at, note.text.clone(), note.color);
                self.notes_rev += 1;
                if let Some(color) = color {
                    // New markers take the colour last chosen.
                    self.marker = color;
                    if let Err(e) = self.update_settings(|s| s.marker = Some(color)) {
                        self.say(format!("Could not save the setting: {e}"));
                    }
                }
                self.say("Note kept".into());
                self.save();
                let at = self.place(at);
                self.emit(EventData::NoteChanged {
                    at,
                    text,
                    color: kept,
                });
                Ok(ResponseResult::Ok)
            }
            Call::NoteRemove(IndexParams { index }) => {
                if index >= self.entry.notes.len() {
                    return Err(refuse("not_found", format!("no note {index}")));
                }
                let note = self.entry.notes.remove(index);
                self.notes_rev += 1;
                self.say(if note.end.is_some() {
                    "Marker removed".into()
                } else {
                    "Note removed".into()
                });
                self.save();
                let at = self.place(note.at);
                self.emit(EventData::NoteRemoved { at });
                Ok(ResponseResult::Ok)
            }
            Call::QuestionAsk(p) => self.ask_call(p),
        }
    }

    /// Where the reader is, as a reply.
    fn moved(&self) -> ResponseResult {
        ResponseResult::Moved {
            at: self.place(self.entry.at),
        }
    }

    /// The page (from 0) `go_to` names, which must name exactly one.
    fn page_to(&self, p: ReaderGoToParams) -> Result<usize, Refused> {
        let count = self.layout.page_count();
        match (p.page, p.chapter, p.at) {
            (Some(page), None, None) if (1..=count).contains(&page) => Ok(page - 1),
            (Some(page), None, None) => Err(refuse(
                "invalid_params",
                format!("page {page} is not in 1..={count}"),
            )),
            (None, Some(i), None) => self
                .chapter_pages
                .get(i)
                .copied()
                .ok_or_else(|| refuse("not_found", format!("no chapter {i}"))),
            (None, None, Some(at)) if at.line < self.doc.lines.len() => Ok(self.layout.page_of(at)),
            (None, None, Some(at)) => Err(refuse(
                "invalid_params",
                format!("line {} is past the end of the book", at.line),
            )),
            _ => Err(refuse("invalid_params", "name one of page, chapter or at")),
        }
    }

    pub(super) fn settings(&self) -> ReaderSettings {
        ReaderSettings {
            direction: if self.rtl {
                Direction::RightToLeft
            } else {
                Direction::LeftToRight
            },
            writing: if self.vertical {
                Writing::Vertical
            } else {
                Writing::Horizontal
            },
            measure: self.measure,
            animation: self.animate,
            note_display: self.note_display,
        }
    }

    fn state(&self) -> ReaderStateInfo {
        let open = self.open_pages();
        let open: Vec<usize> = open
            .filter(|&p| p < self.layout.page_count())
            .map(|p| p + 1)
            .collect();
        ReaderStateInfo {
            book: self.book_info.clone(),
            at: self.place(self.entry.at),
            open,
            spread: self.spread,
            settings: self.settings(),
            bookmarks: self.entry.marks.len(),
            notes: self.entry.notes.len(),
            trail: self.trail.len(),
        }
    }

    /// Changes how the book is set, each as asked; the toasts say what
    /// changed.
    fn set(&mut self, p: ReaderSetParams) -> Result<(), Refused> {
        if let Some(m) = p.measure {
            if !(view::MEASURE_MIN..=view::MEASURE_MAX).contains(&m) {
                return Err(refuse(
                    "invalid_params",
                    format!(
                        "measure {m} is not in {}..={}",
                        view::MEASURE_MIN,
                        view::MEASURE_MAX
                    ),
                ));
            }
            self.set_measure(m);
        }
        if let Some(animate) = p.animation
            && animate != self.animate
        {
            self.toggle_animation();
        }
        if let Some(display) = p.note_display {
            self.set_note_display(display);
        }
        if let Some(writing) = p.writing {
            self.vertical = writing == Writing::Vertical;
            self.entry.writing = Some(writing);
            // Columns run right to left, and so do the pages, unless the
            // same call says otherwise.
            if self.vertical && p.direction.is_none() && !self.rtl {
                self.rtl = true;
                self.entry.direction = Some(Direction::RightToLeft);
            }
            self.say(if self.vertical {
                "Set vertically".into()
            } else {
                "Set across".into()
            });
            self.save();
        }
        if let Some(direction) = p.direction {
            self.rtl = direction == Direction::RightToLeft;
            self.entry.direction = Some(direction);
            self.say(if self.rtl {
                "Pages run right to left".into()
            } else {
                "Pages run left to right".into()
            });
            self.save();
        }
        Ok(())
    }

    fn link_info(&self, i: usize) -> LinkInfo {
        let l = self.doc.links[i];
        let at = Pos {
            line: l.line,
            offset: l.start,
        };
        let end = Pos {
            line: l.line,
            offset: l.end,
        };
        LinkInfo {
            index: i,
            text: self.text_between(at, end),
            at,
            page: self.layout.page_of(at) + 1,
            target: self.place(Pos {
                line: l.target,
                offset: 0,
            }),
            passage: self.passage(l.target),
        }
    }

    fn bookmark_info(&self, i: usize) -> BookmarkInfo {
        let m = self.entry.marks[i];
        BookmarkInfo {
            index: i,
            at: self.place(m.at),
            color: m.color,
            text: self.row_text(m.at),
        }
    }

    fn note_info(&self, i: usize) -> NoteInfo {
        let n = &self.entry.notes[i];
        NoteInfo {
            index: i,
            at: self.place(n.at),
            end: n.end,
            anchor: n.anchor,
            by: n.by,
            text: n.text.clone(),
            question: n.question.clone(),
            quote: n.end.map(|end| self.text_between(n.at, end)),
            color: n.color,
        }
    }

    /// The bookmark on `page`, if there is one.
    pub(super) fn bookmark_on(&self, page: usize) -> Option<usize> {
        self.entry
            .marks
            .iter()
            .position(|m| self.layout.page_of(m.at) == page)
    }

    fn add_bookmark(&mut self, p: BookmarkAddParams) -> Result<ResponseResult, Refused> {
        let count = self.layout.page_count();
        let page = match p.page {
            Some(n) if (1..=count).contains(&n) => n - 1,
            Some(n) => {
                return Err(refuse(
                    "invalid_params",
                    format!("page {n} is not in 1..={count}"),
                ));
            }
            None => self.page(),
        };
        if self.bookmark_on(page).is_some() {
            return Err(refuse(
                "bookmark_exists",
                format!("p.{} is bookmarked already", page + 1),
            ));
        }
        let color = p.color.unwrap_or(self.ribbon);
        let at = self.layout.start_of(page);
        let i = self.entry.marks.partition_point(|m| m.at <= at);
        self.entry.marks.insert(i, Mark { at, color });
        self.say(format!("Bookmarked p.{} ({})", page + 1, color.name()));
        self.save();
        let place = self.place(at);
        self.emit(EventData::BookmarkAdded { at: place, color });
        Ok(ResponseResult::BookmarkAdded {
            bookmark: self.bookmark_info(i),
        })
    }

    fn add_note_call(&mut self, p: NoteAddParams) -> Result<ResponseResult, Refused> {
        let marker = p.anchor == Anchor::Range && p.end.is_some();
        if p.text.trim().is_empty() && !marker {
            return Err(refuse("invalid_params", "a note needs text"));
        }
        let at = p.at.unwrap_or_else(|| self.layout.start_of(self.page()));
        if at.line >= self.doc.lines.len().max(1) {
            return Err(refuse(
                "invalid_params",
                format!("line {} is past the end of the book", at.line),
            ));
        }
        let page = self.layout.page_of(at) + 1;
        let color = p.color.or(p.end.map(|_| self.marker));
        self.add_note(Note {
            at,
            anchor: p.anchor,
            text: p.text.trim().to_string(),
            by: p.by,
            question: p.question,
            end: p.end,
            color,
        });
        self.notes_rev += 1;
        self.say(match (p.by, marker && p.text.trim().is_empty()) {
            (_, true) => format!(
                "Marked ({}); click it to change or remove",
                color.unwrap_or(self.marker).name()
            ),
            (Author::Agent, _) => format!("A note from the agent on p.{page}"),
            (Author::Reader, _) => format!("Note kept on p.{page}"),
        });
        self.save();
        Ok(ResponseResult::NoteAdded { page })
    }

    /// Sends the question, with the open pages, to an agent beside the
    /// book. The agent answers in its own pane, and is asked to keep a
    /// short answer in the book as a note where the question was asked.
    fn ask_call(&mut self, p: QuestionAskParams) -> Result<ResponseResult, Refused> {
        let question = p.question.trim().to_string();
        if question.is_empty() {
            return Err(refuse("invalid_params", "a question needs words"));
        }
        let at = p.at.unwrap_or_else(|| self.layout.start_of(self.page()));
        let agent = herdr::find_agent(self.agent.as_deref()).map_err(|e| {
            self.say(e.clone());
            refuse("no_agent", e)
        })?;
        let prompt = self.prompt(&question, p.anchor, at, p.end);
        self.say(format!("Asking {}…", agent.label));
        let done = self.background.0.clone();
        let label = agent.label.clone();
        std::thread::spawn(move || {
            let status = match herdr::prompt(&agent.target, &prompt) {
                Ok(()) => format!("Asked {}: the answer comes back as a note", agent.label),
                Err(e) => format!("Could not ask {}: {e}", agent.label),
            };
            let _ = done.send(status);
        });
        let place = self.place(at);
        self.emit(EventData::QuestionAsked {
            at: place,
            question,
        });
        Ok(ResponseResult::QuestionSent { agent: label })
    }

    /// Calls on behalf of a key: a refusal is shown as a passing message,
    /// a reply is not needed.
    pub(super) fn act(&mut self, call: Call) {
        if let Err(e) = self.call(call) {
            self.say(e.message);
        }
    }
}
