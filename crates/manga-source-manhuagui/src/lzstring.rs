//! `LZString.decompressFromBase64`, ported from lz-string (MIT).
//!
//! ManHuaGui ships two things compressed this way: the keyword table of a
//! chapter page's packed script, and the chapter list of adult-gated comics
//! (`#__VIEWSTATE`). Works on UTF-16 code units like the JS original.

const KEY_BASE64: &[u8; 65] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";

pub fn decompress_from_base64(input: &str) -> Option<String> {
    if input.is_empty() {
        return None;
    }
    let mut rev = [0u32; 256];
    for (i, &c) in KEY_BASE64.iter().enumerate() {
        rev[c as usize] = i as u32;
    }
    // Unknown characters read as 0, matching how JS treats `undefined & n`.
    let values: Vec<u32> = input.bytes().map(|b| rev[b as usize]).collect();
    decompress(&values, 32).map(|units| String::from_utf16_lossy(&units))
}

struct Bits<'a> {
    values: &'a [u32],
    reset: u32,
    val: u32,
    position: u32,
    index: usize,
}

impl Bits<'_> {
    fn read(&mut self, n: u32) -> u32 {
        let mut bits = 0;
        for i in 0..n {
            let b = self.val & self.position;
            self.position >>= 1;
            if self.position == 0 {
                self.position = self.reset;
                self.val = self.values.get(self.index).copied().unwrap_or(0);
                self.index += 1;
            }
            if b > 0 {
                bits |= 1 << i;
            }
        }
        bits
    }
}

fn decompress(values: &[u32], reset: u32) -> Option<Vec<u16>> {
    let mut bits = Bits { values, reset, val: values.first().copied().unwrap_or(0), position: reset, index: 1 };
    // Entries 0..3 are reserved codes; real entries start at 3.
    let mut dict: Vec<Vec<u16>> = vec![vec![], vec![], vec![]];
    let mut enlarge_in: u32 = 4;
    let mut num_bits: u32 = 3;

    let first = match bits.read(2) {
        0 => bits.read(8) as u16,
        1 => bits.read(16) as u16,
        _ => return Some(vec![]),
    };
    let mut w = vec![first];
    dict.push(w.clone());
    let mut out = w.clone();

    loop {
        if bits.index > values.len() {
            return Some(vec![]);
        }
        let mut c = bits.read(num_bits) as usize;
        match c {
            0 | 1 => {
                let ch = bits.read(if c == 0 { 8 } else { 16 }) as u16;
                dict.push(vec![ch]);
                c = dict.len() - 1;
                enlarge_in -= 1;
            }
            2 => return Some(out),
            _ => {}
        }
        if enlarge_in == 0 {
            enlarge_in = 1 << num_bits;
            num_bits += 1;
        }
        let entry = if let Some(e) = dict.get(c).filter(|e| !e.is_empty()) {
            e.clone()
        } else if c == dict.len() {
            let mut e = w.clone();
            e.push(w[0]);
            e
        } else {
            return None;
        };
        out.extend_from_slice(&entry);
        let mut next = w;
        next.push(entry[0]);
        dict.push(next);
        enlarge_in -= 1;
        w = entry;
        if enlarge_in == 0 {
            enlarge_in = 1 << num_bits;
            num_bits += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_none() {
        assert_eq!(decompress_from_base64(""), None);
    }
}
