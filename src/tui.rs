use crate::hypr::{self, Monitor, fmt_f, parse_mode};
use crate::term::{Key, Term, parse_keys};
use std::fmt::Write as _;
use std::io::Write;
use std::time::{Duration, Instant};

const CONFIRM: Duration = Duration::from_secs(10);

const HELP: &str = "←→↑↓/drag move  tab next  g snap  m mode  +/- scale  r rotate  e toggle  a apply  s save  q quit";

const NORMAL: u8 = 0;
const SELECTED: u8 = 1;
const DIM: u8 = 2;

const U: u8 = 1;
const D: u8 = 2;
const L: u8 = 4;
const R: u8 = 8;
const LIGHT: [char; 16] = [' ', '│', '│', '│', '─', '┘', '┐', '┤', '─', '└', '┌', '├', '─', '┴', '┬', '┼'];
const HEAVY: [char; 16] = [' ', '┃', '┃', '┃', '━', '┛', '┓', '┫', '━', '┗', '┏', '┣', '━', '┻', '┳', '╋'];

pub fn run() -> Result<(), String> {
    let mut mons = hypr::load_monitors()?;
    let mut sel = 0usize;
    let mut status = String::new();
    let term = Term::enter()?;
    let mut frame = String::new();
    let mut buf = [0u8; 256];
    let mut keys = Vec::new();
    let mut live = mons.clone();
    let mut pending: Option<Instant> = None;
    let mut view: Option<View> = None;
    let mut drag: Option<(f64, f64)> = None;

    loop {
        let mut timeout = -1;
        if let Some(deadline) = pending {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                pending = None;
                status = revert(&mut mons, &live, "Timed out. ");
            } else {
                let ms = left.as_millis() as i32;
                let secs = (ms + 999) / 1000;
                status = format!("Keep this layout? y/Enter keep, n/Esc revert ({secs}s)");
                timeout = ms - (secs - 1) * 1000;
            }
        }
        let v = draw(&mut frame, term.size(), &mons, sel, &status, view, drag.is_some());
        view = Some(v);
        let mut out = std::io::stdout().lock();
        out.write_all(frame.as_bytes()).map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        drop(out);

        let Some(n) = term.read(&mut buf, timeout) else {
            if pending.is_some() {
                revert(&mut mons, &live, "");
            }
            return Ok(());
        };
        keys.clear();
        parse_keys(&buf[..n], &mut keys);
        for k in keys.drain(..) {
            if pending.is_some() {
                match k {
                    Key::Char('y') | Key::Enter => {
                        pending = None;
                        live = mons.clone();
                        status = "Kept.".into();
                    }
                    Key::Char('n') | Key::Esc => {
                        pending = None;
                        status = revert(&mut mons, &live, "");
                    }
                    Key::Quit | Key::Char('q') => {
                        revert(&mut mons, &live, "");
                        return Ok(());
                    }
                    _ => {}
                }
                continue;
            }
            let cell = |px: f64| ((px / 10.0).round() as i32 * 10).max(10);
            let sx = |shift: bool| if shift { 10 } else { cell(v.unit) };
            let sy = |shift: bool| if shift { 10 } else { cell(v.unit * 2.0) };
            match k {
                Key::Quit | Key::Esc | Key::Char('q') => return Ok(()),
                Key::Tab if !mons.is_empty() => sel = (sel + 1) % mons.len(),
                Key::BackTab if !mons.is_empty() => sel = (sel + mons.len() - 1) % mons.len(),
                Key::Left(s) => nudge(&mut mons, sel, -sx(s), 0),
                Key::Right(s) => nudge(&mut mons, sel, sx(s), 0),
                Key::Up(s) => nudge(&mut mons, sel, 0, -sy(s)),
                Key::Down(s) => nudge(&mut mons, sel, 0, sy(s)),
                Key::Char('g') if sel < mons.len() => snap_nearest(&mut mons, sel),
                Key::Press(c, r) => {
                    let p = v.to_px(c, r);
                    if r < v.ch {
                        if let Some(i) = hit(&mons, sel, p) {
                            sel = i;
                            drag = Some((p.0 - mons[i].x as f64, p.1 - mons[i].y as f64));
                        }
                    } else if r - v.ch < mons.len() {
                        sel = r - v.ch;
                    }
                }
                Key::Drag(c, r) => {
                    if let (Some((gx, gy)), Some(m)) = (drag, mons.get_mut(sel)) {
                        let (px, py) = v.to_px(c, r);
                        m.x = (px - gx).round() as i32;
                        m.y = (py - gy).round() as i32;
                        snap(&mut mons, sel, v.unit * 1.5);
                    }
                }
                Key::Release => drag = None,
                Key::Char('a') => {
                    hypr::normalize(&mut mons);
                    match hypr::apply(&mons) {
                        Ok(()) => pending = Some(Instant::now() + CONFIRM),
                        Err(e) => status = revert(&mut mons, &live, &format!("Apply failed: {e}. ")),
                    }
                }
                Key::Char('s') => {
                    hypr::normalize(&mut mons);
                    status = hypr::save_profile(&mons)
                        .map_or_else(|e| e, |p| format!("Saved to {}", p.display()));
                }
                k => {
                    let Some(m) = mons.get_mut(sel) else { continue };
                    match k {
                        Key::Char('+' | '=') => m.scale = (m.scale + 0.25).min(4.0),
                        Key::Char('-') => m.scale = (m.scale - 0.25).max(0.5),
                        Key::Char('r') => m.transform = (m.transform + 1) % 4,
                        Key::Char('e') => m.disabled = !m.disabled,
                        Key::Char('m') => next_mode(m),
                        _ => {}
                    }
                }
            }
        }
    }
}

fn revert(mons: &mut Vec<Monitor>, live: &[Monitor], why: &str) -> String {
    *mons = live.to_vec();
    let res = hypr::apply(live).map_or_else(|e| format!("Revert failed: {e}"), |_| "Reverted.".into());
    format!("{why}{res}")
}

fn rect(m: &Monitor) -> [f64; 4] {
    let (w, h) = m.logical_size();
    [m.x as f64, m.y as f64, m.x as f64 + w, m.y as f64 + h]
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] < b[2] - 0.5 && b[0] < a[2] - 0.5 && a[1] < b[3] - 0.5 && b[1] < a[3] - 0.5
}

fn others(mons: &[Monitor], i: usize) -> impl Iterator<Item = [f64; 4]> + '_ {
    mons.iter()
        .enumerate()
        .filter(move |&(j, o)| j != i && !o.disabled)
        .map(|(_, o)| rect(o))
}

fn snap_axis(mons: &mut [Monitor], i: usize, horizontal: bool, threshold: f64) {
    let r = rect(&mons[i]);
    let (k, lo, hi) = if horizontal { (0, r[0], r[2]) } else { (1, r[1], r[3]) };
    let size = hi - lo;
    let mut best: Option<(f64, f64)> = None;
    for o in others(mons, i) {
        let (b0, b1) = (o[k], o[k + 2]);
        for s in [b1, b0 - size, b0, b1 - size] {
            let d = (s - lo).abs();
            if d <= threshold && best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, s));
            }
        }
    }
    if let Some((_, s)) = best {
        let v = s.round() as i32;
        if horizontal { mons[i].x = v } else { mons[i].y = v }
    }
}

fn snap(mons: &mut [Monitor], i: usize, threshold: f64) {
    snap_axis(mons, i, true, threshold);
    snap_axis(mons, i, false, threshold * 2.0);
}

fn snap_nearest(mons: &mut [Monitor], i: usize) {
    let [x, y, x1, y1] = rect(&mons[i]);
    let (w, h) = (x1 - x, y1 - y);
    let mut best: Option<(f64, f64, f64)> = None;
    for o in others(mons, i) {
        let ys = [o[1], o[3] - h, (o[1] + o[3] - h) / 2.0];
        let xs = [o[0], o[2] - w, (o[0] + o[2] - w) / 2.0];
        let near = |c: f64, opts: [f64; 3]| {
            opts.into_iter().min_by(|a, b| (a - c).abs().total_cmp(&(b - c).abs())).unwrap()
        };
        for (cx, cy) in [
            (o[2], near(y, ys)),
            (o[0] - w, near(y, ys)),
            (near(x, xs), o[3]),
            (near(x, xs), o[1] - h),
        ] {
            let cand = [cx, cy, cx + w, cy + h];
            if others(mons, i).any(|b| overlaps(cand, b)) {
                continue;
            }
            let d = (cx - x).hypot(cy - y);
            if best.is_none_or(|(bd, ..)| d < bd) {
                best = Some((d, cx, cy));
            }
        }
    }
    if let Some((_, cx, cy)) = best {
        mons[i].x = cx.round() as i32;
        mons[i].y = cy.round() as i32;
    }
}

fn nudge(mons: &mut [Monitor], i: usize, dx: i32, dy: i32) {
    let Some(m) = mons.get_mut(i) else { return };
    m.x += dx;
    m.y += dy;
    snap_axis(mons, i, dx != 0, (dx.abs() + dy.abs() - 1) as f64);
}

fn hit(mons: &[Monitor], sel: usize, (px, py): (f64, f64)) -> Option<usize> {
    let inside = |i: usize| {
        let r = rect(&mons[i]);
        px >= r[0] && px < r[2] && py >= r[1] && py < r[3]
    };
    (sel < mons.len() && inside(sel))
        .then_some(sel)
        .or_else(|| (0..mons.len()).rev().find(|&i| inside(i)))
}

fn next_mode(m: &mut Monitor) {
    let cur = m.mode_string();
    let modes = &m.available_modes;
    let i = modes
        .iter()
        .position(|s| s.starts_with(&cur))
        .map_or(0, |i| (i + 1) % modes.len());
    if let Some((w, h, hz)) = modes.get(i).and_then(|s| parse_mode(s)) {
        m.width = w;
        m.height = h;
        m.refresh_rate = hz;
    }
}

fn sgr(style: u8) -> &'static str {
    match style {
        SELECTED => "\x1b[0;1;36m",
        DIM => "\x1b[0;2m",
        _ => "\x1b[0m",
    }
}

#[derive(Clone, Copy)]
struct View {
    x0: f64,
    y0: f64,
    unit: f64,
    ox: f64,
    oy: f64,
    cols: usize,
    ch: usize,
}

impl View {
    fn to_px(self, col: usize, row: usize) -> (f64, f64) {
        (
            (col as f64 - self.ox) * self.unit + self.x0,
            (row as f64 - self.oy) * 2.0 * self.unit + self.y0,
        )
    }
}

fn fit(mons: &[Monitor], cols: usize, ch: usize) -> View {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for m in mons {
        let (w, h) = m.logical_size();
        x0 = x0.min(m.x as f64);
        y0 = y0.min(m.y as f64);
        x1 = x1.max(m.x as f64 + w);
        y1 = y1.max(m.y as f64 + h);
    }
    if mons.is_empty() || cols < 3 {
        return View { x0: 0.0, y0: 0.0, unit: 1.0, ox: 0.0, oy: 0.0, cols, ch };
    }
    let unit = ((x1 - x0) / (cols - 1) as f64).max((y1 - y0) / (2 * (ch - 1)) as f64);
    let ox = (cols as f64 - (x1 - x0) / unit) / 2.0;
    let oy = (ch as f64 - (y1 - y0) / unit / 2.0) / 2.0;
    View { x0, y0, unit, ox, oy, cols, ch }
}

fn fits(v: View, mons: &[Monitor]) -> bool {
    mons.iter().all(|m| {
        let [x0, y0, x1, y1] = rect(m);
        (x0 - v.x0) / v.unit + v.ox >= -0.5
            && (x1 - v.x0) / v.unit + v.ox <= v.cols as f64 - 0.5
            && (y0 - v.y0) / v.unit / 2.0 + v.oy >= -0.5
            && (y1 - v.y0) / v.unit / 2.0 + v.oy <= v.ch as f64 - 0.5
    })
}

fn draw(
    f: &mut String,
    (cols, rows): (usize, usize),
    mons: &[Monitor],
    sel: usize,
    status: &str,
    prev: Option<View>,
    locked: bool,
) -> View {
    f.clear();
    f.push_str("\x1b[?2026h");
    let footer = mons.len() + 2;
    let ch = rows.saturating_sub(footer).max(3);
    let mut grid = vec![(' ', NORMAL); cols * ch];

    let fitted = fit(mons, cols, ch);
    let view = match prev {
        Some(p)
            if p.cols == cols
                && p.ch == ch
                && (locked || (fits(p, mons) && fitted.unit > p.unit * 0.7)) =>
        {
            p
        }
        _ => fitted,
    };
    if !mons.is_empty() && cols > 2 {
        let View { x0, y0, unit, ox, oy, .. } = view;
        let cx = |x: f64| ((x - x0) / unit + ox).round() as isize;
        let cy = |y: f64| ((y - y0) / unit / 2.0 + oy).round() as isize;
        let order = (0..mons.len()).filter(|&i| i != sel).chain((sel < mons.len()).then_some(sel));
        let n = cols * ch;
        let (mut mask, mut heavy, mut lit) = (vec![0u8; n], vec![false; n], vec![false; n]);
        let mut text: Vec<Option<(char, u8)>> = vec![None; n];
        let at = |r: isize, c: isize| {
            ((0..ch as isize).contains(&r) && (0..cols as isize).contains(&c))
                .then(|| r as usize * cols + c as usize)
        };
        for i in order {
            let m = &mons[i];
            let [x, y, x1, y1] = rect(m);
            let nudge = |horizontal: bool| -> isize {
                let near = others(mons, i).find_map(|o| {
                    let (edge, start, cell, touch) = if horizontal {
                        (o[2], x, cx(o[2]) == cx(x), o[1] < y1 && o[3] > y)
                    } else {
                        (o[3], y, cy(o[3]) == cy(y), o[0] < x1 && o[2] > x)
                    };
                    (cell && touch && (start - edge).abs() >= 0.5).then_some(start - edge)
                });
                near.map_or(0, |d| if d > 0.0 { 1 } else { -1 })
            };
            let c0 = cx(x) + nudge(true);
            let r0 = cy(y) + nudge(false);
            let c1 = cx(x1).max(c0 + 1);
            let r1 = cy(y1).max(r0 + 1);
            for r in r0 + 1..r1 {
                for c in c0 + 1..c1 {
                    if let Some(k) = at(r, c) {
                        (mask[k], heavy[k], lit[k], text[k]) = (0, false, false, None);
                    }
                }
            }
            let mut line = |r: isize, c: isize, bits: u8| {
                if let Some(k) = at(r, c) {
                    mask[k] |= bits;
                    heavy[k] |= i == sel;
                    lit[k] |= !m.disabled;
                    text[k] = None;
                }
            };
            for c in c0..=c1 {
                let bits = if c > c0 { L } else { 0 } | if c < c1 { R } else { 0 };
                line(r0, c, bits);
                line(r1, c, bits);
            }
            for r in r0..=r1 {
                let bits = if r > r0 { U } else { 0 } | if r < r1 { D } else { 0 };
                line(r, c0, bits);
                line(r, c1, bits);
            }
            let style = if i == sel { SELECTED } else if m.disabled { DIM } else { NORMAL };
            let lr = if r1 - r0 >= 2 { r0 + 1 } else { r0 };
            for (k, g) in m.name.chars().take((c1 - c0 - 1).max(0) as usize).enumerate() {
                if let Some(k) = at(lr, c0 + 1 + k as isize) {
                    text[k] = Some((g, style));
                }
            }
        }
        for k in 0..n {
            grid[k] = match text[k] {
                Some(t) => t,
                None if mask[k] != 0 => {
                    let style = if heavy[k] { SELECTED } else if lit[k] { NORMAL } else { DIM };
                    let set = if heavy[k] { HEAVY } else { LIGHT };
                    (set[mask[k] as usize], style)
                }
                None => (' ', NORMAL),
            };
        }
    }

    let mut cur = NORMAL;
    for (r, row) in grid.chunks(cols).enumerate() {
        let _ = write!(f, "\x1b[{};1H", r + 1);
        let end = row.iter().rposition(|&(g, _)| g != ' ').map_or(0, |e| e + 1);
        for &(g, s) in &row[..end] {
            if s != cur {
                f.push_str(sgr(s));
                cur = s;
            }
            f.push(g);
        }
        f.push_str("\x1b[0m\x1b[K");
        cur = NORMAL;
    }

    let mut line = String::new();
    for (i, m) in mons.iter().enumerate() {
        line.clear();
        let mark = if i == sel { '▶' } else { ' ' };
        if m.disabled {
            let _ = write!(line, "{mark} {}  (disabled)", m.name);
        } else {
            let _ = write!(
                line,
                "{mark} {}  {}  ×{}  @{},{}  t{}",
                m.name,
                m.mode_string(),
                fmt_f(m.scale),
                m.x,
                m.y,
                m.transform
            );
        }
        let style = if i == sel { SELECTED } else if m.disabled { DIM } else { NORMAL };
        row(f, ch + 1 + i, cols, style, &line);
    }
    row(f, ch + mons.len() + 1, cols, SELECTED, status);
    row(f, ch + mons.len() + 2, cols, DIM, HELP);
    f.push_str("\x1b[J\x1b[?2026l");
    view
}

fn row(f: &mut String, r: usize, cols: usize, style: u8, text: &str) {
    let _ = write!(f, "\x1b[{r};1H{}", sgr(style));
    f.extend(text.chars().take(cols));
    f.push_str("\x1b[0m\x1b[K");
}
