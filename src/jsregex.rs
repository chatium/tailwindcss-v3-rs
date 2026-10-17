//! A small backtracking regex engine with JavaScript semantics (non-unicode mode, flags `g`/`i`),
//! enough for the patterns tailwind builds: classes, groups, alternation, greedy and lazy
//! quantifiers, lookahead, backreferences and anchors. Like JS it runs on UTF-16 code units.
//!
//! Single-character repeats use per-input tables (run ends, previous occurrences of the
//! literal that follows), so the usual `[^…]+x` scans are O(1) instead of O(n) per attempt.

#[derive(Debug, Clone)]
enum ClassItem {
    Ch(char),
    Range(char, char),
    Word(bool),
    Digit(bool),
    Space(bool),
}

#[derive(Debug, Clone)]
struct Class {
    items: Vec<ClassItem>,
    negated: bool,
    /// precomputed result for ASCII units (case-sensitive)
    ascii: [bool; 128],
    /// results for non-ASCII units (whitespace, other) when no item mentions a non-ASCII char
    non_ascii: Option<(bool, bool)>,
}

fn is_js_space(c: char) -> bool {
    crate::js::is_js_whitespace(c)
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

impl Class {
    fn new(items: Vec<ClassItem>, negated: bool) -> Class {
        let mut c = Class { items, negated, ascii: [false; 128], non_ascii: None };
        for i in 0..128u8 {
            c.ascii[i as usize] = c.matches(i as char, false);
        }
        let ascii_only = c.items.iter().all(|it| match it {
            ClassItem::Ch(x) => (*x as u32) < 128,
            ClassItem::Range(a, b) => (*a as u32) < 128 && (*b as u32) < 128,
            _ => true,
        });
        if ascii_only {
            c.non_ascii = Some((c.matches('\u{a0}', false), c.matches('\u{e9}', false)));
        }
        c
    }

    #[inline]
    fn matches_unit(&self, u: u16, icase: bool) -> bool {
        if !icase {
            if u < 128 {
                return self.ascii[u as usize];
            }
            if let Some((space, other)) = self.non_ascii {
                let is_space = matches!(u, 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff);
                return if is_space { space } else { other };
            }
        }
        // surrogates never match ASCII-based items; negated classes accept them
        match char::from_u32(u as u32) {
            Some(c) => self.matches(c, icase),
            None => self.negated,
        }
    }

    fn matches(&self, c: char, icase: bool) -> bool {
        let hit = |c: char| {
            self.items.iter().any(|it| match it {
                ClassItem::Ch(x) => *x == c,
                ClassItem::Range(a, b) => *a <= c && c <= *b,
                ClassItem::Word(neg) => is_word(c) != *neg,
                ClassItem::Digit(neg) => c.is_ascii_digit() != *neg,
                ClassItem::Space(neg) => is_js_space(c) != *neg,
            })
        };
        let mut m = hit(c);
        if !m && icase {
            let l = c.to_ascii_lowercase();
            let u = c.to_ascii_uppercase();
            m = (l != c && hit(l)) || (u != c && hit(u));
        }
        m != self.negated
    }
}

#[derive(Debug, Clone)]
enum Node {
    Empty,
    Char(char),
    Any,
    Class(usize),
    Start,
    End,
    WordBoundary(bool),
    Group(Box<Node>, Option<usize>),
    Look(Box<Node>, bool),
    BackRef(usize),
    Concat(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Node>, usize, Option<usize>, bool),
}

struct Parser<'a> {
    s: Vec<char>,
    i: usize,
    groups: usize,
    classes: &'a mut Vec<Class>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.s.get(self.i).copied()
    }

    fn alt(&mut self) -> Node {
        let mut alts = vec![self.concat()];
        while self.peek() == Some('|') {
            self.i += 1;
            alts.push(self.concat());
        }
        if alts.len() == 1 { alts.pop().unwrap() } else { Node::Alt(alts) }
    }

    fn concat(&mut self) -> Node {
        let mut items = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            let atom = self.atom();
            let atom = self.quantifier(atom);
            items.push(atom);
        }
        if items.len() == 1 { items.pop().unwrap() } else if items.is_empty() { Node::Empty } else { Node::Concat(items) }
    }

    fn quantifier(&mut self, atom: Node) -> Node {
        let (min, max) = match self.peek() {
            Some('*') => (0, None),
            Some('+') => (1, None),
            Some('?') => (0, Some(1)),
            Some('{') => {
                // {n}, {n,}, {n,m}; anything else is a literal `{`
                let save = self.i;
                self.i += 1;
                let mut n = String::new();
                while let Some(d) = self.peek().filter(|d| d.is_ascii_digit()) {
                    n.push(d);
                    self.i += 1;
                }
                let mut m: Option<Option<usize>> = None;
                if self.peek() == Some(',') {
                    self.i += 1;
                    let mut mm = String::new();
                    while let Some(d) = self.peek().filter(|d| d.is_ascii_digit()) {
                        mm.push(d);
                        self.i += 1;
                    }
                    m = Some(if mm.is_empty() { None } else { Some(mm.parse().unwrap()) });
                }
                if n.is_empty() || self.peek() != Some('}') {
                    self.i = save;
                    return atom;
                }
                let n: usize = n.parse().unwrap();
                (n, m.unwrap_or(Some(n)))
            }
            _ => return atom,
        };
        self.i += 1;
        let mut greedy = true;
        if self.peek() == Some('?') {
            self.i += 1;
            greedy = false;
        }
        Node::Repeat(Box::new(atom), min, max, greedy)
    }

    fn escape_class(&mut self, c: char) -> Option<ClassItem> {
        Some(match c {
            'w' => ClassItem::Word(false),
            'W' => ClassItem::Word(true),
            'd' => ClassItem::Digit(false),
            'D' => ClassItem::Digit(true),
            's' => ClassItem::Space(false),
            'S' => ClassItem::Space(true),
            _ => return None,
        })
    }

    fn escape_char(c: char) -> char {
        match c {
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'f' => '\x0c',
            'v' => '\x0b',
            '0' => '\0',
            c => c,
        }
    }

    fn atom(&mut self) -> Node {
        let c = self.peek().unwrap();
        self.i += 1;
        match c {
            '.' => Node::Any,
            '^' => Node::Start,
            '$' => Node::End,
            '(' => {
                let node = if self.s[self.i..].starts_with(&['?', ':']) {
                    self.i += 2;
                    Node::Group(Box::new(self.alt()), None)
                } else if self.s[self.i..].starts_with(&['?', '=']) {
                    self.i += 2;
                    Node::Look(Box::new(self.alt()), false)
                } else if self.s[self.i..].starts_with(&['?', '!']) {
                    self.i += 2;
                    Node::Look(Box::new(self.alt()), true)
                } else {
                    self.groups += 1;
                    let idx = self.groups;
                    Node::Group(Box::new(self.alt()), Some(idx))
                };
                // closing paren
                if self.peek() == Some(')') {
                    self.i += 1;
                }
                node
            }
            '[' => {
                let mut negated = false;
                if self.peek() == Some('^') {
                    negated = true;
                    self.i += 1;
                }
                let mut items = Vec::new();
                let mut first = true;
                loop {
                    let Some(c) = self.peek() else { break };
                    if c == ']' && !(first && false) {
                        self.i += 1;
                        break;
                    }
                    first = false;
                    self.i += 1;
                    let lo: Option<char>;
                    if c == '\\' {
                        let e = self.peek().unwrap();
                        self.i += 1;
                        if let Some(it) = self.escape_class(e) {
                            items.push(it);
                            continue;
                        }
                        lo = Some(if e == 'b' { '\x08' } else { Self::escape_char(e) });
                    } else {
                        lo = Some(c);
                    }
                    let lo = lo.unwrap();
                    // range?
                    if self.peek() == Some('-') && self.s.get(self.i + 1).is_some_and(|x| *x != ']') {
                        let save = self.i;
                        self.i += 1;
                        let hc = self.peek().unwrap();
                        self.i += 1;
                        let hi = if hc == '\\' {
                            let e = self.peek().unwrap();
                            self.i += 1;
                            if self.escape_class(e).is_some() {
                                // Annex B: a class escape as range end makes `-` literal
                                self.i = save;
                                items.push(ClassItem::Ch(lo));
                                continue;
                            }
                            Self::escape_char(e)
                        } else {
                            hc
                        };
                        items.push(ClassItem::Range(lo, hi));
                    } else {
                        items.push(ClassItem::Ch(lo));
                    }
                }
                self.classes.push(Class::new(items, negated));
                Node::Class(self.classes.len() - 1)
            }
            '\\' => {
                let e = self.peek().unwrap();
                self.i += 1;
                if let Some(it) = self.escape_class(e) {
                    self.classes.push(Class::new(vec![it], false));
                    return Node::Class(self.classes.len() - 1);
                }
                if e == 'b' {
                    return Node::WordBoundary(false);
                }
                if e == 'B' {
                    return Node::WordBoundary(true);
                }
                if e.is_ascii_digit() && e != '0' {
                    let mut n = e.to_digit(10).unwrap() as usize;
                    while let Some(d) = self.peek().filter(|d| d.is_ascii_digit()) {
                        n = n * 10 + d.to_digit(10).unwrap() as usize;
                        self.i += 1;
                    }
                    return Node::BackRef(n);
                }
                Node::Char(Self::escape_char(e))
            }
            c => Node::Char(c),
        }
    }
}

#[derive(Debug, Clone)]
enum Inst {
    Char(char),
    Any,
    Class(usize),
    Start,
    End,
    WordBoundary(bool),
    Split(usize, usize),
    Jmp(usize),
    Save(usize),
    /// clear capture slots [from, to) at the start of a repeat iteration
    Clear(usize, usize),
    /// run [pc+1, end) as an atomic sub-match; negate?
    Look(usize, bool),
    BackRef(usize),
    /// single-unit repeat
    RepeatOne(usize),
    /// store the current position in loop register r
    SetLoop(usize),
    /// fail if the position did not move since SetLoop(r)
    CheckProgress(usize),
    Match,
}

#[derive(Debug, Clone)]
struct Rep {
    m: Inst,
    min: usize,
    max: Option<usize>,
    greedy: bool,
    /// the literal unit the next instruction requires, if any
    follow: Option<u16>,
}

/// Units a path from an instruction can start with (`any` = cannot filter)
#[derive(Debug, Clone, Copy, PartialEq)]
struct First {
    ascii: u128,
    non_ascii: bool,
    any: bool,
}

impl First {
    const EMPTY: First = First { ascii: 0, non_ascii: false, any: false };
    const ANY: First = First { ascii: 0, non_ascii: false, any: true };
    fn union(self, o: First) -> First {
        First { ascii: self.ascii | o.ascii, non_ascii: self.non_ascii || o.non_ascii, any: self.any || o.any }
    }
    #[inline]
    fn admits(&self, u: Option<u16>) -> bool {
        if self.any {
            return true;
        }
        match u {
            None => false,
            Some(u) if u < 128 => self.ascii & (1u128 << u) != 0,
            Some(_) => self.non_ascii,
        }
    }
}

pub struct Regex {
    /// run-end table key per repeat
    rep_key: Vec<usize>,
    first: Vec<First>,
    prog: Vec<Inst>,
    reps: Vec<Rep>,
    classes: Vec<Class>,
    slots: usize,
    loops: usize,
    icase: bool,
}

struct Compiler<'a> {
    prog: Vec<Inst>,
    reps: Vec<Rep>,
    loops: usize,
    classes: &'a [Class],
}

fn group_range(node: &Node) -> Option<(usize, usize)> {
    fn walk(n: &Node, lo: &mut usize, hi: &mut usize) {
        match n {
            Node::Group(inner, idx) => {
                if let Some(i) = idx {
                    *lo = (*lo).min(*i);
                    *hi = (*hi).max(*i);
                }
                walk(inner, lo, hi);
            }
            Node::Look(inner, _) => walk(inner, lo, hi),
            Node::Concat(v) | Node::Alt(v) => v.iter().for_each(|x| walk(x, lo, hi)),
            Node::Repeat(inner, ..) => walk(inner, lo, hi),
            _ => {}
        }
    }
    let mut lo = usize::MAX;
    let mut hi = 0;
    walk(node, &mut lo, &mut hi);
    if lo == usize::MAX { None } else { Some((lo, hi)) }
}

fn single(node: &Node) -> Option<Inst> {
    match node {
        Node::Char(c) => Some(Inst::Char(*c)),
        Node::Any => Some(Inst::Any),
        Node::Class(i) => Some(Inst::Class(*i)),
        _ => None,
    }
}

impl Compiler<'_> {
    fn emit(&mut self, i: Inst) -> usize {
        self.prog.push(i);
        self.prog.len() - 1
    }

    fn compile(&mut self, node: &Node) {
        match node {
            Node::Empty => {}
            Node::Char(c) => {
                self.emit(Inst::Char(*c));
            }
            Node::Any => {
                self.emit(Inst::Any);
            }
            Node::Class(i) => {
                self.emit(Inst::Class(*i));
            }
            Node::Start => {
                self.emit(Inst::Start);
            }
            Node::End => {
                self.emit(Inst::End);
            }
            Node::WordBoundary(n) => {
                self.emit(Inst::WordBoundary(*n));
            }
            Node::BackRef(n) => {
                self.emit(Inst::BackRef(*n));
            }
            Node::Group(inner, idx) => {
                if let Some(i) = idx {
                    self.emit(Inst::Save(2 * i));
                    self.compile(inner);
                    self.emit(Inst::Save(2 * i + 1));
                } else {
                    self.compile(inner);
                }
            }
            Node::Look(inner, neg) => {
                let at = self.emit(Inst::Look(0, *neg));
                self.compile(inner);
                self.emit(Inst::Match);
                let end = self.prog.len();
                self.prog[at] = Inst::Look(end, *neg);
            }
            Node::Concat(v) => {
                for n in v {
                    self.compile(n);
                }
            }
            Node::Alt(v) => {
                let mut jumps = Vec::new();
                for (k, n) in v.iter().enumerate() {
                    if k + 1 < v.len() {
                        let split = self.emit(Inst::Split(0, 0));
                        self.compile(n);
                        jumps.push(self.emit(Inst::Jmp(0)));
                        let next = self.prog.len();
                        self.prog[split] = Inst::Split(split + 1, next);
                    } else {
                        self.compile(n);
                    }
                }
                let end = self.prog.len();
                for j in jumps {
                    self.prog[j] = Inst::Jmp(end);
                }
            }
            Node::Repeat(inner, min, max, greedy) => {
                if let Some(one) = single(inner) {
                    self.reps.push(Rep { m: one, min: *min, max: *max, greedy: *greedy, follow: None });
                    let id = self.reps.len() - 1;
                    self.emit(Inst::RepeatOne(id));
                    return;
                }
                let clear = group_range(inner);
                let emit_body = |c: &mut Compiler| {
                    if let Some((lo, hi)) = clear {
                        c.emit(Inst::Clear(2 * lo, 2 * hi + 2));
                    }
                    c.compile(inner);
                };
                for _ in 0..*min {
                    emit_body(self);
                }
                match max {
                    Some(m) => {
                        let mut splits = Vec::new();
                        for _ in *min..*m {
                            let s = self.emit(Inst::Split(0, 0));
                            splits.push(s);
                            let r = self.loops;
                            self.loops += 1;
                            self.emit(Inst::SetLoop(r));
                            emit_body(self);
                            self.emit(Inst::CheckProgress(r));
                        }
                        let end = self.prog.len();
                        for s in splits {
                            self.prog[s] = if *greedy { Inst::Split(s + 1, end) } else { Inst::Split(end, s + 1) };
                        }
                    }
                    None => {
                        let r = self.loops;
                        self.loops += 1;
                        let split = self.emit(Inst::Split(0, 0));
                        self.emit(Inst::SetLoop(r));
                        emit_body(self);
                        self.emit(Inst::CheckProgress(r));
                        self.emit(Inst::Jmp(split));
                        let end = self.prog.len();
                        self.prog[split] = if *greedy { Inst::Split(split + 1, end) } else { Inst::Split(end, split + 1) };
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Bt {
    Alt(u32, u32),
    Slot(u32, Option<u32>),
    Loop(u32, u32),
    Greedy { rep: u16, pc: u32, start: u32, count: u32 },
    Lazy { rep: u16, pc: u32, start: u32, count: u32 },
}

const NONE: u32 = u32::MAX;

/// Per-input lookup tables, built lazily.
pub struct Tables<'a> {
    units: &'a [u16],
    /// reusable backtrack stacks, one per lookahead nesting depth
    stacks: Vec<Vec<Bt>>,
    depth: usize,
    /// reusable capture buffers for lookaheads
    slot_pool: Vec<Vec<Option<usize>>>,
    slots: Vec<Option<usize>>,
    loops: Vec<usize>,
    /// run_end[rep][p]: first position ≥ p not matched by the repeat's unit matcher
    run_end: Vec<Vec<u32>>,
    /// (unit, prev[x] = last index ≤ x holding unit, next[x] = first index ≥ x holding unit)
    occ: Vec<(u16, Vec<u32>, Vec<u32>)>,
}

impl<'a> Tables<'a> {
    pub fn new(units: &'a [u16]) -> Tables<'a> {
        Tables { units, stacks: Vec::new(), depth: 0, slot_pool: Vec::new(), slots: Vec::new(), loops: Vec::new(), run_end: Vec::new(), occ: Vec::new() }
    }

    fn occ(&mut self, u: u16) -> usize {
        if let Some(i) = self.occ.iter().position(|(x, ..)| *x == u) {
            return i;
        }
        let n = self.units.len();
        let mut prev = vec![NONE; n + 1];
        let mut last = NONE;
        for i in 0..n {
            if self.units[i] == u {
                last = i as u32;
            }
            prev[i] = last;
        }
        prev[n] = last;
        let mut next = vec![NONE; n + 1];
        let mut nx = NONE;
        for i in (0..n).rev() {
            if self.units[i] == u {
                nx = i as u32;
            }
            next[i] = nx;
        }
        self.occ.push((u, prev, next));
        self.occ.len() - 1
    }
}

impl Regex {
    /// Compiles a JS regex source (as `RegExp.prototype.source` prints it).
    pub fn new(source: &str, icase: bool) -> Regex {
        let mut classes = Vec::new();
        let mut p = Parser { s: source.chars().collect(), i: 0, groups: 0, classes: &mut classes };
        let ast = p.alt();
        let groups = p.groups;
        let mut c = Compiler { prog: Vec::new(), reps: Vec::new(), loops: 0, classes: &classes };
        c.emit(Inst::Save(0));
        c.compile(&ast);
        c.emit(Inst::Save(1));
        c.emit(Inst::Match);
        let _ = c.classes;
        // literal follow units for repeats (case-sensitive only)
        let mut reps = c.reps;
        for (pc, inst) in c.prog.iter().enumerate() {
            if let Inst::RepeatOne(id) = inst {
                if let Some(Inst::Char(ch)) = c.prog.get(pc + 1) {
                    if !icase && (*ch as u32) < 0x10000 {
                        reps[*id].follow = Some(*ch as u32 as u16);
                    }
                }
            }
        }
        // dedupe classes by content so equal matchers share run-end tables
        let mut keys: Vec<String> = Vec::new();
        let mut rep_key = Vec::new();
        for rep in &reps {
            let k = match &rep.m {
                Inst::Class(i) => format!("{:?}{:?}", classes[*i].items, classes[*i].negated),
                other => format!("{other:?}"),
            };
            match keys.iter().position(|x| *x == k) {
                Some(i) => rep_key.push(i),
                None => {
                    keys.push(k);
                    rep_key.push(keys.len() - 1);
                }
            }
        }
        let mut r = Regex { rep_key, first: Vec::new(), prog: c.prog, reps, slots: 2 * (groups + 1), loops: c.loops, classes, icase };
        r.first = (0..r.prog.len()).map(|pc| r.first_of(pc, &mut vec![false; r.prog.len()])).collect();
        r
    }

    fn unit_first(&self, inst: &Inst) -> First {
        if self.icase {
            return First::ANY;
        }
        match inst {
            Inst::Char(c) => {
                let u = *c as u32;
                if u < 128 { First { ascii: 1u128 << u, non_ascii: false, any: false } } else { First { ascii: 0, non_ascii: true, any: false } }
            }
            Inst::Any => {
                let mut a = 0u128;
                for i in 0..128u32 {
                    if i != 10 && i != 13 {
                        a |= 1u128 << i;
                    }
                }
                First { ascii: a, non_ascii: true, any: false }
            }
            Inst::Class(i) => {
                let cl = &self.classes[*i];
                let mut a = 0u128;
                for k in 0..128 {
                    if cl.ascii[k] {
                        a |= 1u128 << k;
                    }
                }
                First { ascii: a, non_ascii: true, any: false }
            }
            _ => First::ANY,
        }
    }

    /// FIRST set of the paths starting at `pc` (`visiting` breaks loops)
    fn first_of(&self, pc: usize, visiting: &mut Vec<bool>) -> First {
        if visiting[pc] {
            return First::EMPTY;
        }
        visiting[pc] = true;
        let f = match &self.prog[pc] {
            Inst::Char(_) | Inst::Any | Inst::Class(_) => self.unit_first(&self.prog[pc]),
            Inst::RepeatOne(id) => {
                let r = &self.reps[*id];
                let m = self.unit_first(&r.m);
                if r.min >= 1 { m } else { m.union(self.first_of(pc + 1, visiting)) }
            }
            Inst::Split(x, y) => {
                let (x, y) = (*x, *y);
                self.first_of(x, visiting).union(self.first_of(y, visiting))
            }
            Inst::Jmp(x) => self.first_of(*x, visiting),
            Inst::Save(_) | Inst::Clear(..) | Inst::SetLoop(_) | Inst::CheckProgress(_) => self.first_of(pc + 1, visiting),
            _ => First::ANY,
        };
        visiting[pc] = false;
        f
    }

    fn eq_unit(&self, a: u16, b: u16) -> bool {
        a == b || (self.icase && a < 128 && b < 128 && (a as u8).to_ascii_lowercase() == (b as u8).to_ascii_lowercase())
    }

    fn one(&self, inst: &Inst, u: u16) -> bool {
        match inst {
            Inst::Char(x) => (*x as u32) < 0x10000 && self.eq_unit(*x as u32 as u16, u),
            Inst::Any => !matches!(u, 0x0a | 0x0d | 0x2028 | 0x2029),
            Inst::Class(i) => self.classes[*i].matches_unit(u, self.icase),
            _ => false,
        }
    }

    /// index of the run-end table for a repeat (tables are shared by repeats with the same
    /// matcher)
    fn run_end(&self, t: &mut Tables, rep: usize) -> usize {
        let key = self.rep_key[rep];
        while t.run_end.len() <= key {
            t.run_end.push(Vec::new());
        }
        if t.run_end[key].is_empty() {
            let n = t.units.len();
            let mut v = vec![0u32; n + 1];
            v[n] = n as u32;
            for i in (0..n).rev() {
                v[i] = if self.one(&self.reps[rep].m, t.units[i]) { v[i + 1] } else { i as u32 };
            }
            t.run_end[key] = v;
        }
        key
    }

    /// Runs the program from `pc` at `pos`; returns the end position on success.
    fn run(&self, t: &mut Tables, pc0: usize, pos0: usize, slots: &mut Vec<Option<usize>>, loops: &mut Vec<usize>) -> Option<usize> {
        let input = t.units;
        if t.stacks.len() <= t.depth {
            t.stacks.push(Vec::with_capacity(64));
        }
        let mut stack: Vec<Bt> = std::mem::take(&mut t.stacks[t.depth]);
        stack.clear();
        let result = self.run_inner(t, pc0, pos0, slots, loops, &mut stack, input);
        let d = t.depth;
        t.stacks[d] = stack;
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run_inner(
        &self,
        t: &mut Tables,
        pc0: usize,
        pos0: usize,
        slots: &mut Vec<Option<usize>>,
        loops: &mut Vec<usize>,
        stack: &mut Vec<Bt>,
        input: &[u16],
    ) -> Option<usize> {
        let mut pc = pc0;
        let mut pos = pos0;
        loop {
            let ok = match &self.prog[pc] {
                Inst::Match => return Some(pos),
                Inst::Char(_) | Inst::Any | Inst::Class(_) => {
                    if pos < input.len() && self.one(&self.prog[pc], input[pos]) {
                        pos += 1;
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Start => {
                    if pos == 0 {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::End => {
                    if pos == input.len() {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::WordBoundary(neg) => {
                    let w = |u: u16| u < 128 && ((u as u8).is_ascii_alphanumeric() || u == b'_' as u16);
                    let a = pos > 0 && w(input[pos - 1]);
                    let b = pos < input.len() && w(input[pos]);
                    if (a != b) != *neg {
                        pc += 1;
                        true
                    } else {
                        false
                    }
                }
                Inst::Split(x, y) => {
                    let u = input.get(pos).copied();
                    if !self.first[*x].admits(u) {
                        pc = *y;
                    } else {
                        if self.first[*y].admits(u) {
                            stack.push(Bt::Alt(*y as u32, pos as u32));
                        }
                        pc = *x;
                    }
                    true
                }
                Inst::Jmp(x) => {
                    pc = *x;
                    true
                }
                Inst::Save(s) => {
                    stack.push(Bt::Slot(*s as u32, slots[*s].map(|x| x as u32)));
                    slots[*s] = Some(pos);
                    pc += 1;
                    true
                }
                Inst::Clear(from, to) => {
                    for s in *from..*to {
                        if slots[s].is_some() {
                            stack.push(Bt::Slot(s as u32, slots[s].map(|x| x as u32)));
                            slots[s] = None;
                        }
                    }
                    pc += 1;
                    true
                }
                Inst::SetLoop(r) => {
                    stack.push(Bt::Loop(*r as u32, loops[*r] as u32));
                    loops[*r] = pos;
                    pc += 1;
                    true
                }
                Inst::CheckProgress(r) => {
                    if loops[*r] == pos {
                        false
                    } else {
                        pc += 1;
                        true
                    }
                }
                Inst::BackRef(n) => {
                    let (a, b) = (slots.get(2 * n).copied().flatten(), slots.get(2 * n + 1).copied().flatten());
                    match (a, b) {
                        (Some(a), Some(b)) if b >= a => {
                            let len = b - a;
                            if a == pos && pos + len <= input.len() {
                                // comparing a span with itself
                                pos += len;
                                pc += 1;
                                true
                            } else if pos + len <= input.len() && (0..len).all(|k| self.eq_unit(input[a + k], input[pos + k])) {
                                pos += len;
                                pc += 1;
                                true
                            } else {
                                false
                            }
                        }
                        _ => {
                            pc += 1;
                            true
                        }
                    }
                }
                Inst::Look(end, neg) => {
                    let mut sub_slots = t.slot_pool.pop().unwrap_or_default();
                    sub_slots.clear();
                    sub_slots.extend_from_slice(slots);
                    t.depth += 1;
                    let r = self.run(t, pc + 1, pos, &mut sub_slots, loops);
                    t.depth -= 1;
                    match (r.is_some(), *neg) {
                        (true, false) => {
                            for (s, v) in sub_slots.iter().enumerate() {
                                if slots[s] != *v {
                                    stack.push(Bt::Slot(s as u32, slots[s].map(|x| x as u32)));
                                    slots[s] = *v;
                                }
                            }
                            t.slot_pool.push(sub_slots);
                            pc = *end;
                            true
                        }
                        (false, true) => {
                            t.slot_pool.push(sub_slots);
                            pc = *end;
                            true
                        }
                        _ => {
                            t.slot_pool.push(sub_slots);
                            false
                        }
                    }
                }
                Inst::RepeatOne(id) => {
                    let rep = &self.reps[*id];
                    let start = pos;
                    let re = self.run_end(t, *id);
                    let avail = t.run_end[re][pos] as usize - pos;
                    if rep.greedy {
                        let n = match rep.max {
                            Some(m) => avail.min(m),
                            None => avail,
                        };
                        if n < rep.min {
                            false
                        } else {
                            if n > rep.min {
                                stack.push(Bt::Greedy { rep: *id as u16, pc: (pc + 1) as u32, start: start as u32, count: n as u32 });
                            }
                            pos = start + n;
                            pc += 1;
                            true
                        }
                    } else if avail < rep.min {
                        false
                    } else {
                        stack.push(Bt::Lazy { rep: *id as u16, pc: (pc + 1) as u32, start: start as u32, count: rep.min as u32 });
                        pos = start + rep.min;
                        pc += 1;
                        true
                    }
                }
            };
            if ok {
                continue;
            }
            loop {
                match stack.pop() {
                    None => return None,
                    Some(Bt::Slot(s, v)) => slots[s as usize] = v.map(|x| x as usize),
                    Some(Bt::Loop(r, v)) => loops[r as usize] = if v == u32::MAX { usize::MAX } else { v as usize },
                    Some(Bt::Alt(x, p)) => {
                        pc = x as usize;
                        pos = p as usize;
                        break;
                    }
                    Some(Bt::Greedy { rep, pc: next, start, count }) => {
                        let (rep, next, start, count) = (rep as usize, next as usize, start as usize, count as usize);
                        let r = &self.reps[rep];
                        let mut c = count - 1;
                        if let Some(f) = r.follow {
                            let oi = t.occ(f);
                            let p = t.occ[oi].1[start + c];
                            if p == NONE || (p as usize) < start + r.min {
                                continue;
                            }
                            c = p as usize - start;
                        }
                        if c > r.min {
                            stack.push(Bt::Greedy { rep: rep as u16, pc: next as u32, start: start as u32, count: c as u32 });
                        }
                        pc = next;
                        pos = start + c;
                        break;
                    }
                    Some(Bt::Lazy { rep, pc: next, start, count }) => {
                        let (rep, next, start, count) = (rep as usize, next as usize, start as usize, count as usize);
                        let r = &self.reps[rep];
                        if r.max.is_some_and(|m| count >= m) {
                            continue;
                        }
                        let re = self.run_end(t, rep);
                        let limit = t.run_end[re][start] as usize;
                        let mut c = count + 1;
                        if start + c > limit {
                            continue;
                        }
                        if let Some(f) = r.follow {
                            let oi = t.occ(f);
                            let q = t.occ[oi].2[start + c];
                            if q == NONE || q as usize > limit {
                                continue;
                            }
                            c = q as usize - start;
                            if r.max.is_some_and(|m| c > m) {
                                continue;
                            }
                        }
                        stack.push(Bt::Lazy { rep: rep as u16, pc: next as u32, start: start as u32, count: c as u32 });
                        pc = next;
                        pos = start + c;
                        break;
                    }
                }
            }
        }
    }

    /// First match at or after `from`: (start, end)
    pub fn exec_at(&self, t: &mut Tables, from: usize) -> Option<(usize, usize)> {
        let mut slots = std::mem::take(&mut t.slots);
        slots.clear();
        slots.resize(self.slots, None);
        let mut loops = std::mem::take(&mut t.loops);
        loops.clear();
        loops.resize(self.loops.max(1), usize::MAX);
        let r = self.exec_inner(t, from, &mut slots, &mut loops);
        t.slots = slots;
        t.loops = loops;
        r
    }

    fn exec_inner(&self, t: &mut Tables, from: usize, slots: &mut Vec<Option<usize>>, loops: &mut Vec<usize>) -> Option<(usize, usize)> {
        let n = t.units.len();
        let mut start = from;
        let f0 = self.first[0];
        while start <= n {
            if !f0.admits(t.units.get(start).copied()) {
                start += 1;
                continue;
            }
            for s in slots.iter_mut() {
                *s = None;
            }
            if let Some(end) = self.run(t, 0, start, slots, loops) {
                return Some((start, end));
            }
            start += 1;
        }
        None
    }

    /// `str.match(re)` with the `g` flag: all match ranges (UTF-16 indices)
    pub fn find_all_units(&self, t: &mut Tables) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut from = 0;
        while from <= t.units.len() {
            match self.exec_at(t, from) {
                Some((s, e)) => {
                    out.push((s, e));
                    from = if e == s { e + 1 } else { e };
                }
                None => break,
            }
        }
        out
    }

    pub fn find_all_str(&self, s: &str) -> Vec<String> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut t = Tables::new(&units);
        self.find_all_units(&mut t).into_iter().map(|(a, b)| String::from_utf16_lossy(&units[a..b])).collect()
    }

    pub fn is_match(&self, s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut t = Tables::new(&units);
        self.exec_at(&mut t, 0).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(re: &str, s: &str) -> Vec<String> {
        Regex::new(re, false).find_all_str(s)
    }

    #[test]
    fn basics() {
        assert_eq!(all(r"a+", "caaab aa"), ["aaa", "aa"]);
        assert_eq!(all(r"a+?", "aaa"), ["a", "a", "a"]);
        assert_eq!(all(r"(?=(a+))\1b", "aaab"), ["aaab"]);
        assert_eq!(all(r"(?:ab)*c", "ababc"), ["ababc"]);
        assert_eq!(all(r"x(?![{([]])", "x] x{] x"), ["x", "x"]);
        assert_eq!(all(r"[^\s]+?\]", "a]b]"), ["a]", "b]"]);
        assert_eq!(all(r"\w+(?:-\w+)*", "foo-bar baz"), ["foo-bar", "baz"]);
        assert_eq!(all(r"^a|b$", "ab"), ["a", "b"]);
        assert_eq!(all(r"[^:]+:", "ab:cd:ef"), ["ab:", "cd:"]);
        assert_eq!(all(r"[a-z]+:x", "ab:cd:x"), ["cd:x"]);
        assert_eq!(all(r"\[[^\s]+?\]x", "[a]b]x"), ["[a]b]x"]);
        assert_eq!(all(r"[^ ]+?\]", "a]b] c]"), ["a]", "b]", "c]"]);
        assert_eq!(all(r"[^<>]+", "😀"), ["😀"]);
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn sizes() {
        eprintln!("Inst {} Bt {}", std::mem::size_of::<super::Inst>(), std::mem::size_of::<super::Bt>());
    }
}
