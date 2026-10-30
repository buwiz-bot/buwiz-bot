// Writes a Scene back to SVG, byte for byte as upstream wrote it. Not an upstream file.

use crate::scene::{Element, Node, Scene};

fn element(out: &mut String, el: &Element) {
    out.push('<');
    out.push_str(&el.name);
    for (k, v) in &el.attrs {
        out.push(' ');
        out.push_str(k);
        out.push_str("=\"");
        out.push_str(v);
        out.push('"');
    }
    if el.empty {
        out.push_str("/>");
        return;
    }
    out.push('>');
    for child in &el.children {
        match child {
            Node::Element(e) => element(out, e),
            Node::Text(t) => out.push_str(t),
        }
    }
    out.push_str("</");
    out.push_str(&el.name);
    out.push('>');
}

pub fn write(scene: &Scene) -> String {
    let mut out = String::new();
    element(&mut out, &scene.root);
    out
}
