//! The regular expressions Hyprland's window rules match classes and titles
//! with, as a small backtracking matcher: literals, `.`, classes, groups,
//! `|`, `* + ? {m,n}` and anchors. A rule matches a whole class or title.

#[derive(Debug)]
enum Atom {
    Char(char),
    Any,
    Class(Vec<(char, char)>, bool),
    Group(Vec<Vec<Piece>>),
    Start,
    End,
}

#[derive(Debug)]
struct Piece {
    atom: Atom,
    min: usize,
    max: usize,
}

/// Whether `pattern` matches all of `text`; `None` if the fake can't read
/// the pattern.
pub fn full_match(pattern: &str, text: &str) -> Option<bool> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let alt = parse_alt(&chars, &mut i)?;
    if i != chars.len() {
        return None;
    }
    let text: Vec<char> = text.chars().collect();
    Some(match_alt(&alt, &text, 0, &mut |j| j == text.len()))
}

fn parse_alt(p: &[char], i: &mut usize) -> Option<Vec<Vec<Piece>>> {
    let mut alts = vec![parse_seq(p, i)?];
    while p.get(*i) == Some(&'|') {
        *i += 1;
        alts.push(parse_seq(p, i)?);
    }
    Some(alts)
}

fn parse_seq(p: &[char], i: &mut usize) -> Option<Vec<Piece>> {
    let mut seq = Vec::new();
    while let Some(&c) = p.get(*i) {
        if c == '|' || c == ')' {
            break;
        }
        *i += 1;
        let atom = match c {
            '(' => {
                if p.get(*i) == Some(&'?') && p.get(*i + 1) == Some(&':') {
                    *i += 2;
                }
                let inner = parse_alt(p, i)?;
                if p.get(*i) != Some(&')') {
                    return None;
                }
                *i += 1;
                Atom::Group(inner)
            }
            '[' => parse_class(p, i)?,
            '.' => Atom::Any,
            '^' => Atom::Start,
            '$' => Atom::End,
            '\\' => {
                let e = *p.get(*i)?;
                *i += 1;
                escape(e)
            }
            '*' | '+' | '?' | '{' => return None,
            c => Atom::Char(c),
        };
        let (mut min, mut max) = (1, 1);
        let quantified = matches!(p.get(*i), Some('*' | '+' | '?' | '{'));
        match p.get(*i) {
            Some('*') => (min, max) = (0, usize::MAX),
            Some('+') => (min, max) = (1, usize::MAX),
            Some('?') => (min, max) = (0, 1),
            Some('{') => {
                let close = p[*i..].iter().position(|&c| c == '}')? + *i;
                let body: String = p[*i + 1..close].iter().collect();
                let (lo, hi) = match body.split_once(',') {
                    Some((lo, "")) => (lo.parse().ok()?, usize::MAX),
                    Some((lo, hi)) => (lo.parse().ok()?, hi.parse().ok()?),
                    None => (body.parse().ok()?, body.parse().ok()?),
                };
                (min, max) = (lo, hi);
                *i = close;
            }
            _ => {}
        }
        if quantified {
            *i += 1;
            if p.get(*i) == Some(&'?') {
                *i += 1; // lazy: same set of matches for a full match
            }
        }
        seq.push(Piece { atom, min, max });
    }
    Some(seq)
}

fn escape(e: char) -> Atom {
    let digit = vec![('0', '9')];
    let word = vec![('a', 'z'), ('A', 'Z'), ('0', '9'), ('_', '_')];
    let space = vec![(' ', ' '), ('\t', '\r')];
    match e {
        'd' => Atom::Class(digit, false),
        'D' => Atom::Class(digit, true),
        'w' => Atom::Class(word, false),
        'W' => Atom::Class(word, true),
        's' => Atom::Class(space, false),
        'S' => Atom::Class(space, true),
        'n' => Atom::Char('\n'),
        't' => Atom::Char('\t'),
        c => Atom::Char(c),
    }
}

fn parse_class(p: &[char], i: &mut usize) -> Option<Atom> {
    let negated = p.get(*i) == Some(&'^');
    if negated {
        *i += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let mut c = *p.get(*i)?;
        *i += 1;
        if c == ']' && !first {
            return Some(Atom::Class(ranges, negated));
        }
        first = false;
        if c == '\\' {
            c = *p.get(*i)?;
            *i += 1;
            if let Atom::Class(more, false) = escape(c) {
                ranges.extend(more);
                continue;
            }
        }
        if p.get(*i) == Some(&'-') && p.get(*i + 1).is_some_and(|&e| e != ']') {
            let end = p[*i + 1];
            *i += 2;
            ranges.push((c, end));
        } else {
            ranges.push((c, c));
        }
    }
}

fn match_alt(alts: &[Vec<Piece>], s: &[char], i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    alts.iter().any(|seq| match_seq(seq, s, i, k))
}

fn match_seq(seq: &[Piece], s: &[char], i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    match seq.split_first() {
        None => k(i),
        Some((piece, rest)) => match_rep(piece, 0, rest, s, i, k),
    }
}

fn match_rep(
    piece: &Piece,
    count: usize,
    rest: &[Piece],
    s: &[char],
    i: usize,
    k: &mut dyn FnMut(usize) -> bool,
) -> bool {
    if count < piece.max
        && match_atom(&piece.atom, s, i, &mut |j| {
            (j != i || count < piece.min) && match_rep(piece, count + 1, rest, s, j, k)
        })
    {
        return true;
    }
    count >= piece.min && match_seq(rest, s, i, k)
}

fn match_atom(atom: &Atom, s: &[char], i: usize, k: &mut dyn FnMut(usize) -> bool) -> bool {
    match atom {
        Atom::Char(c) => s.get(i) == Some(c) && k(i + 1),
        Atom::Any => s.get(i).is_some_and(|&c| c != '\n') && k(i + 1),
        Atom::Class(ranges, negated) => {
            s.get(i)
                .is_some_and(|&c| ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated)
                && k(i + 1)
        }
        Atom::Group(alts) => match_alt(alts, s, i, k),
        Atom::Start => i == 0 && k(i),
        Atom::End => i == s.len() && k(i),
    }
}
