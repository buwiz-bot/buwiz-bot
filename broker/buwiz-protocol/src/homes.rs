//! The homes a Bot's computer can be made on, for the app and the broker together: each kind in the
//! order the broker offers it, and what a person calls it. The app shows these before the broker has
//! answered; the broker's own names (`ProviderKind::label()`) are checked against them in its tests.

/// (kind, what a person calls it), in the broker's order.
pub const HOMES: [(&str, &str); 5] = [
    ("docker", "Docker on this Mac"),
    ("ec2", "AWS EC2"),
    ("e2b", "E2B"),
    ("boat", "boat.dev"),
    ("remote", "A remote computer"),
];
