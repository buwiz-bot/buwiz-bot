// Port of src/text/entities.ts.

use std::collections::HashMap;
use std::sync::OnceLock;

const NAMED: &str = "amp:38,lt:60,gt:62,quot:34,apos:39,nbsp:160,shy:173,iexcl:161,cent:162,pound:163,curren:164,yen:165,sect:167,copy:169,laquo:171,not:172,reg:174,deg:176,plusmn:177,sup2:178,sup3:179,micro:181,para:182,middot:183,sup1:185,raquo:187,frac14:188,frac12:189,frac34:190,iquest:191,times:215,divide:247,ndash:8211,mdash:8212,lsquo:8216,rsquo:8217,sbquo:8218,ldquo:8220,rdquo:8221,bdquo:8222,dagger:8224,Dagger:8225,bull:8226,hellip:8230,permil:8240,prime:8242,Prime:8243,lsaquo:8249,rsaquo:8250,euro:8364,trade:8482,larr:8592,uarr:8593,rarr:8594,darr:8595,harr:8596,lArr:8656,uArr:8657,rArr:8658,dArr:8659,hArr:8660,forall:8704,part:8706,exist:8707,empty:8709,nabla:8711,isin:8712,notin:8713,ni:8715,prod:8719,sum:8721,minus:8722,radic:8730,infin:8734,and:8743,or:8744,cap:8745,cup:8746,int:8747,there4:8756,sim:8764,cong:8773,asymp:8776,ne:8800,equiv:8801,le:8804,ge:8805,sub:8834,sup:8835,sube:8838,supe:8839,oplus:8853,otimes:8855,perp:8869,sdot:8901,loz:9674,spades:9824,clubs:9827,hearts:9829,diams:9830,check:10003,cross:10007,AMP:38,LT:60,GT:62,QUOT:34,lsqb:91,rsqb:93,lcub:123,rcub:125,verbar:124,VerticalLine:124,midast:42,DiacriticalGrave:96,Hat:94,UnderBar:95,equals:61,colon:58,semi:59,num:35,percnt:37,lpar:40,rpar:41,lbrack:91,rbrack:93,lbrace:123,rbrace:125,vert:124,sol:47,bsol:92,quest:63,excl:33,ast:42,plus:43,comma:44,period:46,dollar:36,commat:64,grave:96,hat:94,lowbar:95,tilde:126,Alpha:913,Beta:914,Gamma:915,Delta:916,Epsilon:917,Zeta:918,Eta:919,Theta:920,Iota:921,Kappa:922,Lambda:923,Mu:924,Nu:925,Xi:926,Omicron:927,Pi:928,Rho:929,Sigma:931,Tau:932,Upsilon:933,Phi:934,Chi:935,Psi:936,Omega:937,alpha:945,beta:946,gamma:947,delta:948,epsilon:949,zeta:950,eta:951,theta:952,iota:953,kappa:954,lambda:955,mu:956,nu:957,xi:958,omicron:959,pi:960,rho:961,sigmaf:962,sigma:963,tau:964,upsilon:965,phi:966,chi:967,psi:968,omega:969";

fn lookup(name: &str) -> Option<char> {
    static NAMES: OnceLock<HashMap<&'static str, char>> = OnceLock::new();
    NAMES
        .get_or_init(|| {
            NAMED
                .split(',')
                .map(|pair| {
                    let (k, v) = pair.split_once(':').unwrap();
                    (k, char::from_u32(v.parse().unwrap()).unwrap())
                })
                .collect()
        })
        .get(name)
        .copied()
}

fn code_point(n: f64) -> Option<char> {
    if n > 0.0 && n <= 1114111.0 && !(55296.0..=57343.0).contains(&n) {
        char::from_u32(n as u32)
    } else {
        None
    }
}

const OPEN: &str = "\u{fb02}\u{b0}";
const CLOSE: &str = "\u{b6}\u{df}";

// Mermaid's placeholders run from `ﬂ°` to `¶ß`, with a second `°` marking a numeric code. Like
// Mermaid, turn them back into entities, so that each code is decoded once.
fn restore_placeholders(text: &str) -> String {
    let mut out = String::new();
    let mut last = 0;
    let mut from = text.find(OPEN);
    while let Some(f) = from {
        let Some(stop) = text[f + OPEN.len()..].find('\u{b6}').map(|p| p + f + OPEN.len()) else { break };
        if text[stop + 2..].starts_with('\u{df}') {
            let numeric = text[f + OPEN.len()..].starts_with('\u{b0}');
            let body_start = f + OPEN.len() + if numeric { 2 } else { 0 };
            out.push_str(&text[last..f]);
            out.push_str(if numeric { "&#" } else { "&" });
            if body_start <= stop {
                out.push_str(&text[body_start..stop]);
            }
            out.push(';');
            last = stop + CLOSE.len();
        }
        from = text[stop + 2..].find(OPEN).map(|p| p + stop + 2);
    }
    if last == 0 {
        return text.to_string();
    }
    out.push_str(&text[last..]);
    out
}

/// `text.replace(/ﬂ°°?(\w+)¶ß/g, '#$1;')`.
pub fn written_codes(text: &str) -> String {
    if !text.contains(CLOSE) {
        return text.to_string();
    }
    let mut out = String::new();
    let mut rest = text;
    while let Some(f) = rest.find(OPEN) {
        let mut p = f + OPEN.len();
        if rest[p..].starts_with('\u{b0}') {
            p += 2;
        }
        let w = rest[p..].bytes().take_while(|b| b.is_ascii_alphanumeric() || *b == b'_').count();
        if w > 0 && rest[p + w..].starts_with(CLOSE) {
            out.push_str(&rest[..f]);
            out.push('#');
            out.push_str(&rest[p..p + w]);
            out.push(';');
            rest = &rest[p + w + CLOSE.len()..];
        } else {
            let step = f + OPEN.chars().next().unwrap().len_utf8();
            out.push_str(&rest[..step]);
            rest = &rest[step..];
        }
    }
    out.push_str(rest);
    out
}

/// Turns Mermaid's `#35;` / `#quot;` codes (as hidden by encodeEntities) and HTML entities into characters.
pub fn decode_entities(text: &str) -> String {
    let text = if text.contains(CLOSE) { restore_placeholders(text) } else { text.to_string() };
    if !text.contains('&') {
        return text;
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'&' {
            i += 1;
            continue;
        }
        let mut replaced: Option<(usize, Option<char>)> = None;
        if b.get(i + 1) == Some(&b'#') {
            let d = b[i + 2..].iter().take_while(|c| c.is_ascii_digit()).count();
            if d > 0 && b.get(i + 2 + d) == Some(&b';') {
                let n: f64 = text[i + 2..i + 2 + d].parse().unwrap_or(f64::INFINITY);
                replaced = Some((i + 3 + d, code_point(n)));
            } else if matches!(b.get(i + 2), Some(b'x' | b'X')) {
                let h = b[i + 3..].iter().take_while(|c| c.is_ascii_hexdigit()).count();
                if h > 0 && b.get(i + 3 + h) == Some(&b';') {
                    let n = crate::js::parse_int(&text[i + 3..i + 3 + h], 16);
                    replaced = Some((i + 4 + h, code_point(n)));
                }
            }
        } else {
            let w = b[i + 1..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
            if w > 0 && b.get(i + 1 + w) == Some(&b';') {
                replaced = Some((i + 2 + w, lookup(&text[i + 1..i + 1 + w])));
            }
        }
        match replaced {
            Some((end, Some(c))) => {
                out.push_str(&text[last..i]);
                out.push(c);
                last = end;
                i = end;
            }
            Some((end, None)) => i = end,
            None => i += 1,
        }
    }
    out.push_str(&text[last..]);
    out
}
