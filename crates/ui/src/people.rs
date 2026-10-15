//! People in the words: who a capture names, drawn where they are named.
//!
//! A capture can mention someone by `@handle` (`@casey`, or
//! `@casey-lane` for the full name) or just by writing their name
//! ("Reply to Casey"). Either way the row shows them **inline** — a small
//! avatar and their first name, or their full name when the words wrote
//! it out — and a tap opens their profile ([`PersonProfile`]), whether
//! they are in the org or only a contact. Every person is in one context
//! (`Contact::context`), so naming them says where the work belongs.
//!
//! The same reading lifts `#tags` into badges and `@contexts` into
//! chips; [`marks`] is the one parser for all three.

use architect_ui::prelude::*;
use dioxus::prelude::*;

use crate::routes::Route;
use crate::stores;

/// Someone a capture can be about: a contact, or an org member who is
/// not (yet) one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    /// Their full name.
    pub name: String,
    pub email: String,
    /// The contact behind them — where a tap goes. `None` for a member
    /// with no contact yet.
    pub contact_id: Option<String>,
    /// The one context they are in (`Contact::context`).
    pub context: Option<String>,
}

impl Person {
    /// The first name, as written.
    #[must_use]
    pub fn first(&self) -> &str {
        self.name.split_whitespace().next().unwrap_or_default()
    }

    /// The last name, when they have one.
    fn last(&self) -> Option<&str> {
        let mut words = self.name.split_whitespace();
        words.next();
        words.last()
    }

    /// What an `@handle` must be to name them, with the label it reads
    /// as: the first name alone, or the full name hyphenated or run
    /// together (`@casey-lane`, `@caseylane`).
    fn label_for_handle(&self, handle: &str) -> Option<String> {
        let h = handle.to_lowercase();
        if h == self.first().to_lowercase() {
            return Some(self.first().to_owned());
        }
        let full: Vec<String> = self
            .name
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        (self.last().is_some() && (h == full.join("-") || h == full.join("")))
            .then(|| self.name.clone())
    }
}

/// One piece of a line as the row draws it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    Text(String),
    /// A person, drawn as an inline avatar with `label`.
    Person {
        person: Person,
        label: String,
    },
}

/// A line of a capture, read for what it is marked with. `segs` is the
/// line as the row shows it: `#tags` and `@contexts` lifted out into
/// badges, people drawn in place.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Marks {
    pub segs: Vec<Seg>,
    pub tags: Vec<String>,
    /// Written ones first, then the contexts of the people named.
    pub contexts: Vec<String>,
    pub people: Vec<Person>,
}

impl Marks {
    /// The line as plain words, people by their labels.
    #[must_use]
    pub fn text(&self) -> String {
        self.segs
            .iter()
            .map(|s| match s {
                Seg::Text(t) => t.as_str(),
                Seg::Person { label, .. } => label.as_str(),
            })
            .collect()
    }

    /// Fold another line's marks in (its segments are not kept).
    pub fn merge(&mut self, other: Marks) {
        for t in other.tags {
            push_unique(&mut self.tags, t);
        }
        for c in other.contexts {
            push_unique(&mut self.contexts, c);
        }
        for p in other.people {
            push_unique(&mut self.people, p);
        }
    }
}

fn push_unique<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if !v.contains(&x) {
        v.push(x);
    }
}

/// Builds segments, keeping one space between words.
#[derive(Default)]
struct Line {
    segs: Vec<Seg>,
    buf: String,
}

impl Line {
    fn space(&mut self) {
        if !self.buf.is_empty() || !self.segs.is_empty() {
            self.buf.push(' ');
        }
    }
    fn word(&mut self, w: &str) {
        self.space();
        self.buf.push_str(w);
    }
    /// A person, then whatever punctuation followed their name.
    fn person(&mut self, person: Person, label: String, tail: &str) {
        self.space();
        if !self.buf.is_empty() {
            self.segs.push(Seg::Text(std::mem::take(&mut self.buf)));
        }
        self.segs.push(Seg::Person { person, label });
        self.buf.push_str(tail);
    }
    fn finish(mut self) -> Vec<Seg> {
        let rest = self.buf.trim_end().to_owned();
        if !rest.is_empty() {
            self.segs.push(Seg::Text(rest));
        }
        // A lifted mark can leave a trailing space on the text before it.
        if let Some(Seg::Text(t)) = self.segs.last_mut() {
            let trimmed = t.trim_end().to_owned();
            *t = trimmed;
        }
        self.segs
    }
}

/// Read a line for its marks (see [`Marks`]).
#[must_use]
pub fn marks(line: &str, known: &[Person]) -> Marks {
    let mut out = Marks::default();
    let mut named: Vec<Person> = Vec::new();
    let mut l = Line::default();
    let words: Vec<&str> = line.split_whitespace().collect();
    let is_name = |s: &str| {
        s.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/')
            && s.chars().any(char::is_alphabetic)
    };
    // Matched without trailing punctuation, which stays in the text.
    let split = |w: &'_ str| -> (String, String) {
        let core = w.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '_');
        (core.to_owned(), w[core.len()..].to_owned())
    };
    let mut i = 0;
    while i < words.len() {
        let word = words[i];
        i += 1;
        let (core, tail) = split(word);
        if let Some(tag) = core.strip_prefix('#').filter(|t| is_name(t)) {
            push_unique(&mut out.tags, tag.to_owned());
            continue;
        }
        if let Some(at) = core.strip_prefix('@').filter(|t| is_name(t)) {
            if let Some((p, label)) = known
                .iter()
                .find_map(|p| p.label_for_handle(at).map(|l| (p.clone(), l)))
            {
                push_unique(&mut named, p.clone());
                l.person(p, label, &tail);
            } else {
                push_unique(&mut out.contexts, at.to_owned());
            }
            continue;
        }
        // A name said plainly ("Reply to Casey", "Casey Lane's notes"):
        // capitalised, as names are written, so "sam" the word never
        // matches. The full name when the words wrote it out.
        let (bare, poss) = match core.strip_suffix("'s") {
            Some(b) => (b.to_owned(), "'s"),
            None => (core.clone(), ""),
        };
        let person = (bare.chars().count() >= 3 && bare.starts_with(char::is_uppercase))
            .then(|| known.iter().find(|p| p.first() == bare))
            .flatten();
        if let Some(p) = person {
            let (next_core, next_tail) = words.get(i).map(|w| split(w)).unwrap_or_default();
            let next_bare = next_core.strip_suffix("'s").unwrap_or(&next_core);
            if poss.is_empty() && p.last().is_some_and(|last| last == next_bare) {
                // "Casey Lane" — both words are the name.
                let next_poss = if next_core.ends_with("'s") { "'s" } else { "" };
                i += 1;
                push_unique(&mut named, p.clone());
                l.person(
                    p.clone(),
                    p.name.clone(),
                    &format!("{next_poss}{next_tail}"),
                );
            } else {
                push_unique(&mut named, p.clone());
                l.person(p.clone(), p.first().to_owned(), &format!("{poss}{tail}"));
            }
            continue;
        }
        l.word(word);
    }
    for p in &named {
        // Not when a #tag already says it ("#client" and @client).
        if let Some(c) = p.context.as_ref().filter(|c| !out.tags.contains(c)) {
            push_unique(&mut out.contexts, c.clone());
        }
    }
    out.people = named;
    out.segs = l.finish();
    out
}

/// Everyone a capture in this org might mention: its contacts, then
/// members who are not one yet — less the person signed in. Empty until
/// they load.
pub fn use_known_people(slug: Memo<Option<String>>) -> Vec<Person> {
    let account = use_context::<Signal<Option<crate::auth::ActiveAccount>>>();
    let contacts = stores::use_contact_list();
    let members = use_resource(move || {
        let slug = slug();
        let token = account.read().as_ref().map(|a| a.token.clone());
        async move {
            match slug {
                Some(s) => crate::feeds::fetch_org_members(&s, token.unwrap_or_default())
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            }
        }
    });
    // Worked out on each render, not memoised: the contact list is a
    // store read, and a memo over it missed the load landing.
    {
        let me = account.read().as_ref().map(|a| a.email.to_lowercase());
        let mut people: Vec<Person> = Vec::new();
        let mut add = |p: Person| {
            let email = p.email.to_lowercase();
            if p.name.trim().is_empty() || (!email.is_empty() && me.as_deref() == Some(&email)) {
                return;
            }
            let same = |q: &Person| {
                (!email.is_empty() && q.email.eq_ignore_ascii_case(&email))
                    || q.name.eq_ignore_ascii_case(&p.name)
            };
            if !people.iter().any(same) {
                people.push(p);
            }
        };
        if let Some(rows) = contacts.value() {
            for (_, c) in rows.iter().filter(|(_, c)| !c.archived) {
                add(Person {
                    name: c.full_name.trim().to_owned(),
                    email: c.primary_email().unwrap_or_default().to_owned(),
                    contact_id: Some(c.id.clone()),
                    context: Some(c.context().to_owned()),
                });
            }
        }
        if let Some(list) = &*members.read() {
            for m in list {
                add(Person {
                    name: m.name.trim().to_owned(),
                    email: m.email.clone(),
                    contact_id: None,
                    context: None,
                });
            }
        }
        people
    }
}

/// A line drawn with its people in place: text, and each person as an
/// avatar and name that opens their profile.
#[component]
pub fn MarkedLine(segs: Vec<Seg>) -> Element {
    rsx! {
        for (n, seg) in segs.into_iter().enumerate() {
            match seg {
                Seg::Text(t) => rsx! { span { key: "{n}", "{t}" } },
                Seg::Person { person, label } => rsx! {
                    MentionChip { key: "{n}", person, label }
                },
            }
        }
    }
}

/// A person named in a line: their avatar and name, inline with the
/// words. Opens their profile (or the directory, for a member who is
/// not a contact yet). It sits inside tappable rows, so it is a span
/// that stops the row's tap rather than a nested button.
#[component]
pub fn MentionChip(person: Person, label: String) -> Element {
    let nav = use_navigator();
    let to = person
        .contact_id
        .clone()
        .map_or(Route::ContactsRoute {}, |id| Route::PersonRoute { id });
    let title = match &person.context {
        Some(c) => format!("{} · @{c}", person.name),
        None => person.name.clone(),
    };
    rsx! {
        span {
            role: "link",
            tabindex: "0",
            "data-testid": "mention",
            title: "{title}",
            // Inline, not inline-flex: the name keeps the line's own
            // baseline and size, and the avatar is centred on it.
            class: "mx-px whitespace-nowrap rounded-full bg-foreground/[0.07] py-px pl-px pr-1.5 font-medium text-foreground [box-decoration-break:clone] hover:bg-foreground/[0.12] active:bg-foreground/[0.15]",
            onclick: move |e| {
                e.stop_propagation();
                nav.push(to.clone());
            },
            span { class: "mr-1 inline-block align-middle -translate-y-px",
                task_ui_core::avatar::Avatar { name: person.name.clone(), email: person.email.clone(), size: 18 }
            }
            "{label}"
        }
    }
}

/// `/people/:id` — one person: who they are, the context they are in,
/// how to reach them, and the captures that name them.
#[component]
pub fn PersonProfile(id: String) -> Element {
    let contacts = stores::use_contact_list();
    let inbox = stores::use_inbox_list();
    let nav = use_navigator();
    let Some(c) = contacts.value().and_then(|rows| {
        rows.iter()
            .find(|(_, c)| c.id == id)
            .map(|(_, c)| c.clone())
    }) else {
        return if contacts.is_waiting() {
            rsx! { crate::states::LoadingState {} }
        } else {
            rsx! {
                div { class: "mx-auto max-w-md px-6 py-24 text-center",
                    Text { variant: TextVariant::Muted, "No one by that id in this directory." }
                }
            }
        };
    };
    let me = Person {
        name: c.full_name.clone(),
        email: c.primary_email().unwrap_or_default().to_owned(),
        contact_id: Some(c.id.clone()),
        context: Some(c.context().to_owned()),
    };
    let role = [c.title.clone(), c.organization.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ");
    let known = [me.clone()];
    let mentions: Vec<(String, Vec<Seg>)> = inbox
        .value()
        .map(|rows| {
            rows.iter()
                .filter(|(_, it)| it.status != inbox_proto::InboxItem::STATUS_PROCESSED)
                .filter_map(|(_, it)| {
                    let (title, _) = crate::process::split_title_body(&it.body);
                    let m = marks(&title, &known);
                    (!m.people.is_empty()).then(|| (it.id.clone(), m.segs))
                })
                .collect()
        })
        .unwrap_or_default();
    let emails: Vec<String> = c.email_list().into_iter().map(str::to_owned).collect();
    let phones: Vec<String> = c.phone_list().into_iter().map(str::to_owned).collect();
    let context = c.context().to_owned();
    let notes = c.notes.clone().unwrap_or_default();

    rsx! {
        div { class: "mx-auto flex w-full max-w-2xl flex-col gap-6 px-4 pt-6 pb-10 sm:px-8",
            "data-testid": "person-profile",
            div { class: "flex flex-col items-center gap-3 text-center",
                task_ui_core::avatar::Avatar { name: me.name.clone(), email: me.email.clone(), size: 88 }
                div { class: "flex flex-col gap-1",
                    h1 { class: "text-[26px] font-semibold tracking-[-0.01em] text-foreground", "{c.full_name}" }
                    if !role.is_empty() {
                        p { class: "text-[15px] text-muted-foreground", "{role}" }
                    }
                }
                span {
                    "data-testid": "person-context",
                    class: "inline-flex items-center gap-1 rounded-full border border-border px-2.5 py-0.5 text-[13px] text-foreground/80",
                    architect_ui::lucide_dioxus::DoorOpen { size: 12 }
                    "{context}"
                }
            }
            if !emails.is_empty() || !phones.is_empty() {
                div { class: "flex flex-col overflow-hidden rounded-2xl bg-card",
                    for e in emails {
                        a { key: "{e}", href: "mailto:{e}", class: "flex min-h-12 items-center gap-3 border-b border-border/60 px-4 text-[15px] text-foreground last:border-0",
                            architect_ui::lucide_dioxus::Mail { size: 16 }
                            "{e}"
                        }
                    }
                    for p in phones {
                        a { key: "{p}", href: "tel:{p}", class: "flex min-h-12 items-center gap-3 border-b border-border/60 px-4 text-[15px] text-foreground last:border-0",
                            architect_ui::lucide_dioxus::Phone { size: 16 }
                            "{p}"
                        }
                    }
                }
            }
            if !notes.trim().is_empty() {
                p { class: "whitespace-pre-wrap text-[15px] leading-6 text-foreground/90", "{notes}" }
            }
            div { class: "flex flex-col gap-1",
                h2 { class: "px-1 pb-1 text-[12px] font-semibold uppercase tracking-[0.06em] text-muted-foreground/80",
                    "Mentioned in"
                }
                if mentions.is_empty() {
                    p { class: "px-1 text-[14px] text-muted-foreground", "No captures name them yet." }
                }
                for (iid, segs) in mentions {
                    button {
                        key: "{iid}",
                        r#type: "button",
                        class: "flex min-h-11 w-full items-center gap-3 rounded-xl px-2 py-2 text-left text-[15px] leading-6 text-foreground hover:bg-accent/40 active:bg-accent/60",
                        onclick: move |_| {
                            nav.push(Route::InboxRoute {});
                        },
                        span { class: "flex size-7 shrink-0 items-center justify-center rounded-[8px] bg-foreground/[0.07] text-muted-foreground",
                            architect_ui::lucide_dioxus::Inbox { size: 14 }
                        }
                        span { class: "min-w-0 flex-1", MarkedLine { segs } }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cast() -> Vec<Person> {
        [
            ("Sam Reeve", Some("studio")),
            ("Casey Lane", Some("client")),
            ("Jordan", None),
        ]
        .iter()
        .map(|(n, c)| Person {
            name: (*n).to_owned(),
            email: String::new(),
            contact_id: Some((*n).to_lowercase()),
            context: c.map(str::to_owned),
        })
        .collect()
    }

    fn labels(m: &Marks) -> Vec<&str> {
        m.segs
            .iter()
            .filter_map(|s| match s {
                Seg::Person { label, .. } => Some(label.as_str()),
                Seg::Text(_) => None,
            })
            .collect()
    }

    #[test]
    fn tags_and_contexts_become_badges_and_leave_the_text() {
        let m = marks("Descale the espresso machine #chore @home.", &cast());
        assert_eq!(m.text(), "Descale the espresso machine");
        assert_eq!(m.tags, vec!["chore"]);
        assert_eq!(m.contexts, vec!["home"]);
        assert!(m.people.is_empty());
        // Not marks: a number, an email address, a lone sign.
        let m = marks("Fix #12 for a@b.test # now", &cast());
        assert!(m.tags.is_empty() && m.contexts.is_empty());
        assert_eq!(m.text(), "Fix #12 for a@b.test # now");
    }

    #[test]
    fn a_handle_is_drawn_in_place_by_first_name() {
        let m = marks("Ask @sam, then ship it @office", &cast());
        assert_eq!(m.text(), "Ask Sam, then ship it");
        assert_eq!(labels(&m), vec!["Sam"]);
        assert!(
            matches!(&m.segs[..], [Seg::Text(a), Seg::Person { .. }, Seg::Text(b)] if a == "Ask " && b == ", then ship it"),
            "{:?}",
            m.segs
        );
        assert_eq!(
            m.contexts,
            vec!["office", "studio"],
            "a handle nobody owns is a context; a person brings theirs"
        );
    }

    #[test]
    fn the_full_name_shows_when_the_words_write_it() {
        let m = marks("Send @casey-lane the mix", &cast());
        assert_eq!(labels(&m), vec!["Casey Lane"]);
        let m = marks("Fold in Casey Lane's notes", &cast());
        assert_eq!(labels(&m), vec!["Casey Lane"]);
        assert_eq!(m.text(), "Fold in Casey Lane's notes");
        let m = marks("Casey sent notes", &cast());
        assert_eq!(labels(&m), vec!["Casey"]);
        assert_eq!(m.text(), "Casey sent notes");
    }

    #[test]
    fn a_word_is_a_name_only_when_capitalised() {
        let m = marks("and sam the word, jordan too", &cast());
        assert!(m.people.is_empty());
        assert_eq!(m.text(), "and sam the word, jordan too");
    }
}
