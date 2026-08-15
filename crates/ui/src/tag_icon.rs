//! Renders a [`tag_proto::TagIcon`] — the icon a Tag carries.
//!
//! `Named(key)` maps the key to a `dioxus-icons` Lucide component;
//! unknown keys fall back to a neutral tag glyph. `Svg(markup)` renders
//! the pasted SVG inline. The curated [`CURATED_ICON_KEYS`] is what the
//! picker offers; expand it (and add a match arm) to grow the set — or
//! the user pastes their own SVG for anything not listed.

use dioxus::prelude::*;
use dioxus_icons::lucide;
use tag_proto::TagIcon;

/// Render a tag's icon at `size` px.
#[component]
pub fn TagIconView(icon: TagIcon, #[props(default = 16)] size: u32) -> Element {
    match &icon {
        TagIcon::Svg(svg) => rsx! {
            span { class: "inline-flex shrink-0 items-center", dangerous_inner_html: "{svg}" }
        },
        TagIcon::Named(key) => named_icon(key, size),
    }
}

/// The picker's curated set of named icons (Lucide keys), grouped loosely
/// by theme. Keep in sync with [`named_icon`]'s match arms.
pub const CURATED_ICON_KEYS: &[&str] = &[
    // food & drink
    "utensils",
    "coffee",
    "pizza",
    "wine",
    "apple",
    "cake",
    "cooking-pot",
    // health & movement
    "dumbbell",
    "bike",
    "footprints",
    "activity",
    "heart",
    "stethoscope",
    "pill",
    "bed",
    // work & study
    "briefcase",
    "code",
    "laptop",
    "presentation",
    "calendar",
    "clock",
    "book-open",
    "graduation-cap",
    "school",
    "pencil",
    "lightbulb",
    // leisure & media
    "palette",
    "music",
    "film",
    "gamepad",
    "camera",
    "headphones",
    // travel & places
    "car",
    "plane",
    "train",
    "bus",
    "map-pin",
    "mountain",
    "tent",
    "house",
    // money & errands
    "shopping-cart",
    "gift",
    "dollar-sign",
    "credit-card",
    "phone",
    "mail",
    // markers & people
    "bell",
    "star",
    "flag",
    "bookmark",
    "users",
    "user",
    "baby",
    "dog",
    "cat",
    // nature & misc
    "leaf",
    "sun",
    "moon",
    "droplet",
    "flame",
    "sprout",
    "tree-pine",
    "wrench",
    "hammer",
    "rocket",
    "trophy",
    "target",
    "brain",
    "tag",
];

/// Map a curated key to its Lucide component; unknown keys → `tag`.
#[allow(clippy::too_many_lines)]
fn named_icon(key: &str, size: u32) -> Element {
    match key {
        "utensils" => rsx! { lucide::Utensils { size } },
        "coffee" => rsx! { lucide::Coffee { size } },
        "pizza" => rsx! { lucide::Pizza { size } },
        "wine" => rsx! { lucide::Wine { size } },
        "apple" => rsx! { lucide::Apple { size } },
        "cake" => rsx! { lucide::Cake { size } },
        "cooking-pot" => rsx! { lucide::CookingPot { size } },
        "dumbbell" => rsx! { lucide::Dumbbell { size } },
        "bike" => rsx! { lucide::Bike { size } },
        "footprints" => rsx! { lucide::Footprints { size } },
        "activity" => rsx! { lucide::Activity { size } },
        "heart" => rsx! { lucide::Heart { size } },
        "stethoscope" => rsx! { lucide::Stethoscope { size } },
        "pill" => rsx! { lucide::Pill { size } },
        "bed" => rsx! { lucide::Bed { size } },
        "briefcase" => rsx! { lucide::Briefcase { size } },
        "code" => rsx! { lucide::Code { size } },
        "laptop" => rsx! { lucide::Laptop { size } },
        "presentation" => rsx! { lucide::Presentation { size } },
        "calendar" => rsx! { lucide::Calendar { size } },
        "clock" => rsx! { lucide::Clock { size } },
        "book-open" => rsx! { lucide::BookOpen { size } },
        "graduation-cap" => rsx! { lucide::GraduationCap { size } },
        "school" => rsx! { lucide::School { size } },
        "pencil" => rsx! { lucide::Pencil { size } },
        "lightbulb" => rsx! { lucide::Lightbulb { size } },
        "palette" => rsx! { lucide::Palette { size } },
        "music" => rsx! { lucide::Music { size } },
        "film" => rsx! { lucide::Film { size } },
        "gamepad" => rsx! { lucide::Gamepad2 { size } },
        "camera" => rsx! { lucide::Camera { size } },
        "headphones" => rsx! { lucide::Headphones { size } },
        "car" => rsx! { lucide::Car { size } },
        "plane" => rsx! { lucide::Plane { size } },
        "train" => rsx! { lucide::TrainFront { size } },
        "bus" => rsx! { lucide::Bus { size } },
        "map-pin" => rsx! { lucide::MapPin { size } },
        "mountain" => rsx! { lucide::Mountain { size } },
        "tent" => rsx! { lucide::Tent { size } },
        "house" => rsx! { lucide::House { size } },
        "shopping-cart" => rsx! { lucide::ShoppingCart { size } },
        "gift" => rsx! { lucide::Gift { size } },
        "dollar-sign" => rsx! { lucide::DollarSign { size } },
        "credit-card" => rsx! { lucide::CreditCard { size } },
        "phone" => rsx! { lucide::Phone { size } },
        "mail" => rsx! { lucide::Mail { size } },
        "bell" => rsx! { lucide::Bell { size } },
        "star" => rsx! { lucide::Star { size } },
        "flag" => rsx! { lucide::Flag { size } },
        "bookmark" => rsx! { lucide::Bookmark { size } },
        "users" => rsx! { lucide::Users { size } },
        "user" => rsx! { lucide::User { size } },
        "baby" => rsx! { lucide::Baby { size } },
        "dog" => rsx! { lucide::Dog { size } },
        "cat" => rsx! { lucide::Cat { size } },
        "leaf" => rsx! { lucide::Leaf { size } },
        "sun" => rsx! { lucide::Sun { size } },
        "moon" => rsx! { lucide::Moon { size } },
        "droplet" => rsx! { lucide::Droplet { size } },
        "flame" => rsx! { lucide::Flame { size } },
        "sprout" => rsx! { lucide::Sprout { size } },
        "tree-pine" => rsx! { lucide::TreePine { size } },
        "wrench" => rsx! { lucide::Wrench { size } },
        "hammer" => rsx! { lucide::Hammer { size } },
        "rocket" => rsx! { lucide::Rocket { size } },
        "trophy" => rsx! { lucide::Trophy { size } },
        "target" => rsx! { lucide::Target { size } },
        "brain" => rsx! { lucide::Brain { size } },
        _ => rsx! { lucide::Tag { size } },
    }
}
