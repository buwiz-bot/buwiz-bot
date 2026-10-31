//! A screen's root: the screen's view under an error boundary that sends
//! its failures to the host. Without one, a Rust component's panic is only
//! logged (nsh reports a JS app's errors to the view's error handler, not
//! a Rust root's), so the mini app would sit blank with no banner.

use std::sync::Arc;

use nsh_core::element::{
    AnyElement, Children, Component, IntoElement, NoEvent, PropError, PropSpec, Props, Root, Scope,
    Slot, TsTypes, catch_error, view, AnyValue,
};

use crate::Bridge;

type View = Arc<dyn Fn(Bridge) -> AnyElement + Send + Sync>;

/// The screen made by `view` with `bridge`, as the root its
/// [`crate::ScreenDef::build`] returns. A panic under it (its view, a
/// binding, a listener, an effect; tasks: [`Bridge::spawn`]) shows the mini
/// app's error banner, as a TSX app's uncaught error does.
pub fn root<E: IntoElement>(bridge: Bridge, view: impl Fn(Bridge) -> E + Send + Sync + 'static) -> Root {
    let view: View = Arc::new(move |b| view(b).into_element());
    let screen = Arc::new(Screen { bridge, view });
    Root::new::<Boundary>(move || ScreenProps(Some(screen.clone())))
}

struct Screen {
    bridge: Bridge,
    view: View,
}

/// The props of both components: the screen, set from Rust only.
#[derive(Clone, Default)]
struct ScreenProps(Option<Arc<Screen>>);

impl PartialEq for ScreenProps {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

impl Props for ScreenProps {
    fn spec(_: &mut TsTypes) -> Vec<PropSpec> {
        Vec::new()
    }

    fn initial() -> Self {
        ScreenProps(None)
    }

    fn set(&mut self, name: &str, _: Option<AnyValue>) -> Result<bool, PropError> {
        Err(PropError(format!("unknown prop `{name}`")))
    }

    fn set_slot(&mut self, _: &str, _: Option<Slot>) -> bool {
        false
    }

    fn set_children(&mut self, _: Children) -> bool {
        false
    }
}

/// The boundary. The screen's view is a component of its own under it, so
/// a failure of that component's own code goes to it too (a failure goes
/// to the boundary above the failing component).
struct Boundary;

impl Component for Boundary {
    type Props = ScreenProps;
    type Event = NoEvent;

    fn view(props: ScreenProps, _: Scope<NoEvent>) -> impl IntoElement {
        let Some(screen) = props.0 else { return view().into_element() };
        let bridge = screen.bridge.clone();
        catch_error(
            move || Body::element(ScreenProps(Some(screen.clone()))),
            move |error, _| {
                bridge.fail(&error.message);
                view()
            },
        )
    }
}

/// The screen's own view.
struct Body;

impl Component for Body {
    type Props = ScreenProps;
    type Event = NoEvent;

    fn view(props: ScreenProps, _: Scope<NoEvent>) -> impl IntoElement {
        match props.0 {
            Some(screen) => (screen.view)(screen.bridge.clone()),
            None => view().into_element(),
        }
    }
}

/// The message of a panic's payload.
pub(crate) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_else(|| "a panic".to_owned())
}
