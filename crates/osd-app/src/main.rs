//! osd の GUI。ブリッジ API（osd-server）を叩いて、本筋と付箋を描く。

use rune::tok::{self, type_};
use rune::{AppSpec, Cx, El, col, text};

#[derive(Default)]
struct State {}

fn view(_s: &State, _cx: &mut Cx<State>) -> El {
    let t = tok::theme(true);
    col()
        .w_full()
        .h_full()
        .gap(12.0)
        .fill(t.bg)
        .child(text("osd").font(type_::TITLE).color(t.text))
        .child(text("ブリッジに繋いでいません").color(t.text_secondary))
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[],
        ..AppSpec::new("osd", (900.0, 700.0), State::default, view)
    });
}
