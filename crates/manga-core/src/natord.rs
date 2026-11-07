use std::cmp::Ordering;

/// Natural ordering: `第2话` < `第10话`, `p9.jpg` < `p10.jpg`. Case-insensitive.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = take_number(&mut a);
                let nb = take_number(&mut b);
                // Compare by magnitude: strip leading zeros, then length, then digits.
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut s = String::new();
    while let Some(c) = it.peek().copied().filter(char::is_ascii_digit) {
        s.push(c);
        it.next();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_numbers_by_value() {
        let mut v = vec!["p10.jpg", "p2.jpg", "p1.jpg", "P3.jpg", "p02.jpg"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["p1.jpg", "p2.jpg", "p02.jpg", "P3.jpg", "p10.jpg"]);
    }

    #[test]
    fn chinese_chapters() {
        assert_eq!(natural_cmp("第9话", "第10话"), Ordering::Less);
    }
}
