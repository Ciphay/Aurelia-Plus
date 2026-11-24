pub const SLOT_SET_PRESENCE: u16 = 6;

fn clean(s: &str) -> String {
    s.chars().filter(|c| *c != '\0').collect()
}

pub fn rich_presence_kv(pairs: &[(String, String)]) -> Vec<u8> {
    let mut out = vec![0x00];
    out.extend_from_slice(b"RP\0");
    for (k, v) in pairs {
        out.push(0x01);
        out.extend_from_slice(clean(k).as_bytes());
        out.push(0);
        out.extend_from_slice(clean(v).as_bytes());
        out.push(0);
    }
    out.extend_from_slice(&[0x08, 0x08]);
    out
}

pub fn parse_presence_payload(p: &[u8]) -> Option<Vec<(String, String)>> {
    let mut o = 0usize;
    let u16at = |o: &mut usize| -> Option<usize> {
        let b = p.get(*o..*o + 2)?;
        *o += 2;
        Some(u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let n = u16at(&mut o)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let mut text = || -> Option<String> {
            let len = u16at(&mut o)?;
            let b = p.get(o..o + len)?;
            o += len;
            String::from_utf8(b.to_vec()).ok()
        };
        let k = text()?;
        let v = text()?;
        out.push((k, v));
    }
    (o == p.len()).then_some(out)
}

#[cfg(test)]
#[path = "presence_tests.rs"]
mod tests;
