//! Command palette search: which commands match a query, best first.

use imager_command::CommandSpec;

/// How well `needle` matches `text`, or `None` if it does not. Both are
/// lower case. The tiers, best first: the whole text, its start, the start
/// of a word in it, the initials of its words, anywhere inside it, and
/// finally its letters in order with gaps (when `fuzzy` is allowed).
fn text_score(needle: &str, text: &str, fuzzy: bool) -> Option<i32> {
    if text == needle {
        return Some(100);
    }
    if text.starts_with(needle) {
        return Some(90);
    }
    let words = || {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
    };
    if text
        .match_indices(needle)
        .any(|(at, _)| !text[..at].ends_with(char::is_alphanumeric))
    {
        return Some(80);
    }
    let initials: String = words().filter_map(|w| w.chars().next()).collect();
    if needle.chars().count() >= 2 && initials.starts_with(needle) {
        return Some(70);
    }
    if text.contains(needle) {
        return Some(60);
    }
    if !fuzzy {
        return None;
    }
    // Letters in order: the tighter the span they cover, the better.
    let mut rest = text.char_indices();
    let mut first = None;
    let mut last = 0;
    for wanted in needle.chars().filter(|c| !c.is_whitespace()) {
        let (at, _) = rest.find(|&(_, c)| c == wanted)?;
        first.get_or_insert(at);
        last = at;
    }
    let gaps = (last + 1 - first?).saturating_sub(needle.len());
    Some(40 - gaps.min(20) as i32)
}

/// Best score of `needle` over everything a command can be found by.
/// Titles count most, then the ImageJ name, the id and the summary; the
/// summary is long enough that scattered letters in it mean nothing.
fn spec_score(needle: &str, spec: &CommandSpec) -> Option<i32> {
    let field = |text: &str, penalty: i32, fuzzy: bool| {
        text_score(needle, &text.to_lowercase(), fuzzy).map(|score| score - penalty)
    };
    [
        field(&spec.title, 0, true),
        spec.ijm_name
            .as_deref()
            .and_then(|name| field(name, 2, true)),
        field(&spec.id, 10, true),
        field(&spec.summary, 35, false),
    ]
    .into_iter()
    .flatten()
    .max()
}

/// How well a command matches a query, or `None` if it does not. The query
/// may match as one phrase, or word by word in any order.
pub fn score(query: &str, spec: &CommandSpec) -> Option<i32> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let phrase = spec_score(&query, spec);
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.len() < 2 {
        return phrase;
    }
    let by_word = words
        .iter()
        .map(|word| spec_score(word, spec))
        .collect::<Option<Vec<i32>>>()
        .and_then(|scores| scores.into_iter().min())
        // A phrase match of the same quality is the better one.
        .map(|score| score - 3);
    phrase.max(by_word)
}

/// The commands matching `query`, best first. An empty query lists every
/// command by title.
pub fn search<'a>(
    specs: impl Iterator<Item = &'a CommandSpec>,
    query: &str,
) -> Vec<&'a CommandSpec> {
    let mut hits: Vec<(i32, &CommandSpec)> = specs
        .filter_map(|spec| score(query, spec).map(|score| (score, spec)))
        .collect();
    hits.sort_by(|(a_score, a), (b_score, b)| {
        b_score
            .cmp(a_score)
            .then_with(|| a.title.len().cmp(&b.title.len()))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });
    if query.trim().is_empty() {
        hits.sort_by(|(_, a), (_, b)| a.title.cmp(&b.title).then_with(|| a.id.cmp(&b.id)));
    }
    hits.into_iter().map(|(_, spec)| spec).collect()
}
