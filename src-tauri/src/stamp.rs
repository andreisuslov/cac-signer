//! Visible signature in Acrobat's usual layout: the signer's name large on the left,
//! "Digitally signed by", the name and the date small on the right.
//! Text is set in Helvetica, a standard PDF font, so nothing has to be embedded.

/// Helvetica advance widths for ASCII 32..=126, in 1/1000 em.
const WIDTHS: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // space../
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 0..?
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // @..O
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // P.._
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // `..o
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // p..~
];

fn width(text: &str, size: f32) -> f32 {
    let units: u32 = text
        .chars()
        .map(|c| match c as u32 {
            code @ 32..=126 => WIDTHS[(code - 32) as usize] as u32,
            _ => 556,
        })
        .sum();
    units as f32 * size / 1000.0
}

/// Breaks `text` between characters so each line fits in `max` points.
fn wrap(text: &str, max: f32, size: f32) -> Vec<String> {
    let mut lines = vec![String::new()];
    for c in text.chars() {
        let line = lines.last_mut().unwrap();
        line.push(c);
        if line.chars().count() > 1 && width(line, size) > max {
            line.pop();
            lines.push(c.to_string());
        }
    }
    lines
}

/// PDF literal string body. Helvetica here only covers ASCII, so anything else becomes '?'.
fn literal(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '(' | ')' | '\\' => format!("\\{c}"),
            ' '..='~' => c.to_string(),
            _ => "?".into(),
        })
        .collect()
}

/// Number for a content stream: at most three decimals, no exponent.
pub fn num(v: f32) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// Content stream for a `w` x `h` appearance that uses the font resource /Helv.
pub fn appearance(w: f32, h: f32, name: &str, date: &str) -> Vec<u8> {
    let col = w / 2.0 - 4.0;
    let mut ops = String::from("q 0 g\n");

    let chars: Vec<char> = name.chars().collect();
    let half = chars.len().div_ceil(2);
    let left: [String; 2] = [chars[..half].iter().collect(), chars[half..].iter().collect()];
    let widest = left.iter().map(|l| width(l, 1.0)).fold(1.0, f32::max);
    let big = (col / widest).min(h / 2.3);
    // Two lines of capitals (0.72 em tall), 1.15 em apart, centered in the box.
    let first = h / 2.0 + big * (0.72 + 1.15) / 2.0 - big * 0.72;
    for (i, line) in left.iter().enumerate() {
        let y = first - i as f32 * big * 1.15;
        ops += &format!("BT /Helv {} Tf 3 {} Td ({}) Tj ET\n", num(big), num(y), literal(line));
    }

    let mut small = 6.0;
    let lines = loop {
        let mut lines = vec!["Digitally signed by".to_string()];
        lines.extend(wrap(name, col, small));
        lines.push(format!("Date: {date}"));
        if lines.len() as f32 * small * 1.15 <= h || small <= 3.0 {
            break lines;
        }
        small -= 0.25;
    };
    let lead = small * 1.15;
    let mut y = h / 2.0 + lead * lines.len() as f32 / 2.0 - small;
    for line in &lines {
        ops += &format!(
            "BT /Helv {} Tf {} {} Td ({}) Tj ET\n",
            num(small),
            num(w / 2.0 + 2.0),
            num(y),
            literal(line)
        );
        y -= lead;
    }
    ops += "Q";
    ops.into_bytes()
}
