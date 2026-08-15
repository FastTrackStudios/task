//! [`Nutrition`] — the shared macro shape — plus the full
//! [`Recipe`] wire model (cooklang `.cook` files as typed wire
//! values). Both live in this wasm-clean proto so the web UI can
//! bind to the recipe model + [`crate::service::CookbookService`]
//! client directly without pulling the vault-backed `cookbook`
//! crate. The native `cookbook` crate re-exports these so the
//! existing `cookbook::Recipe` / `cookbook::model::*` paths keep
//! working.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

/// Per-unit nutrition. Lives on a `pantry::PantryItem` (the
/// wiki page for "Flour" carries `nutritionPerUnit` so any
/// recipe using `@flour{...}` can be aggregated at mealprep
/// time). Kept in this proto crate as the shared nutrition shape —
/// consumers (`pantry`, `intake`, `fitness`) all reference
/// `cookbook::Nutrition`. Derives `architect::JsonField` so
/// downstream crates can use it as a `#[architect(json)]`
/// column directly (no `DailyTarget`-style wrapper needed).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
pub struct Nutrition {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub calories: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "proteinG")]
    pub protein_g: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "carbsG")]
    pub carbs_g: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "fatG")]
    pub fat_g: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "fiberG")]
    pub fiber_g: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "sugarG")]
    pub sugar_g: Option<f64>,
}

/// `Vec<String>` newtype — JSON column. Steps / cookware /
/// nested_recipes / tags all share this shape.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for StringList {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for StringList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for StringList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// `Vec<Ingredient>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Ingredients(pub Vec<Ingredient>);

impl Ingredients {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<Ingredient>> for Ingredients {
    fn from(v: Vec<Ingredient>) -> Self {
        Self(v)
    }
}

impl FromIterator<Ingredient> for Ingredients {
    fn from_iter<I: IntoIterator<Item = Ingredient>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for Ingredients {
    type Target = Vec<Ingredient>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// A timer mentioned in a step (`~name{10%minutes}`). `seconds` is the
/// countdown length; `display` keeps the written form for the label.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct RecipeTimer {
    /// Optional label — `~rest{…}` → `"rest"`; `None` for a bare `~{…}`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    /// Duration in whole seconds — what a countdown counts down.
    pub seconds: u32,
    /// Written form, e.g. `"10 minutes"`, for the timer label.
    pub display: String,
}

/// One cooking step: readable text (ingredient / cookware / timer
/// names kept inline, not stripped to `·`) plus the timers it
/// mentions, each ready to wire straight to a countdown.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct CookStep {
    pub text: String,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub timers: Vec<RecipeTimer>,
    /// The cooklang section this step came from (`= Prep`), or
    /// `None` for steps written before the first `=` heading.
    /// Cook mode walks the recipe one section at a time, so this
    /// is what splits gathering from prep from the cook itself.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub section: Option<String>,
    /// Asides hanging off this step — cooklang's `> …` blocks.
    ///
    /// These are not instructions and must not be numbered as though
    /// they were. "Don't brown it, it turns bitter" is a warning about
    /// the step you just read, and presenting it as the next thing to
    /// do actively misleads: the cook looks for something to *do* and
    /// finds a caution about something already done.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub notes: Vec<String>,

    /// Cookware this step reaches for, positioned in the text the same
    /// way [`CookStep::ingredients`] are — so "wide pan" in a step can
    /// point at the wide pan in the equipment list rather than being a
    /// word that happens to match one.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub cookware: Vec<StepCookware>,

    /// Which ingredients this step calls for, and where each sits in
    /// [`CookStep::text`].
    ///
    /// A recipe written in prose can only tell a reader "add the
    /// garlic" and leave them to scroll back for how much. Because
    /// cooklang marks its ingredients, we know the step, the name, the
    /// quantity, and the exact span of characters to point at — so a
    /// reading view can put the amount right where the eye already is.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub ingredients: Vec<StepIngredient>,

    /// Wikilinks written in this step, with the span of markup each one
    /// occupies. Cooklang treats `[[…]]` as ordinary text, so without
    /// this the brackets render verbatim.
    ///
    /// `#[facet(default)]` because the web client ships ahead of the
    /// server: without it a new client decoding an old server's recipe
    /// fails the whole decode plan, and the cookbook goes blank rather
    /// than merely un-linked.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[facet(default)]
    pub links: Vec<StepLink>,
}

/// `Vec<RecipeImage>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct RecipeImages(pub Vec<RecipeImage>);

impl RecipeImages {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<RecipeImage>> for RecipeImages {
    fn from(v: Vec<RecipeImage>) -> Self {
        Self(v)
    }
}

impl FromIterator<RecipeImage> for RecipeImages {
    fn from_iter<I: IntoIterator<Item = RecipeImage>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for RecipeImages {
    type Target = Vec<RecipeImage>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// An image found beside a recipe file.
///
/// Discovered by filename convention rather than declared in the
/// recipe — `Pasta.jpg` is the dish, `Pasta.0.jpg` belongs to the first
/// step. That's the convention cooklang-find, CookCLI, cooklang-chef
/// and cooklang-obsidian all landed on independently, so a cookbook
/// carried between them keeps its pictures.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct RecipeImage {
    /// Wiki-relative path, forward-slash separated.
    pub path: String,
    /// `None` for the dish's own image; `Some(n)` for step `n`.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "stepIndex")]
    pub step_index: Option<u32>,
}

/// A wikilink as it appears inside one step's text.
///
/// The span covers the whole `[[Target|alias]]{2}` run, markup and all,
/// so a reader can draw the display text over it. Without this the
/// brackets reach the screen — and a cook following a recipe should
/// never be shown link syntax.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct StepLink {
    /// The page this points at.
    pub target: String,
    /// What to render in place of the markup.
    pub display: String,
    /// True for the braced form, which pulls in another recipe. False
    /// for a bare `[[concept]]`, which is just a reference.
    #[serde(
        skip_serializing_if = "std::ops::Not::not",
        default,
        rename = "isRecipe"
    )]
    pub is_recipe: bool,
    /// Byte offset of the opening `[` within [`CookStep::text`].
    pub start: u32,
    /// Byte length of the whole run, markup included.
    pub len: u32,
}

/// A piece of cookware as it appears inside one step's text.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct StepCookware {
    /// Index into [`Recipe::cookware`].
    pub index: u32,
    /// The text written in the step.
    pub name: String,
    /// Byte offset of `name` within [`CookStep::text`], on a char
    /// boundary — the name is spliced in whole.
    pub start: u32,
    /// Byte length of `name`.
    pub len: u32,
}

/// An ingredient as it appears inside one step's text.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Facet)]
pub struct StepIngredient {
    /// Index into [`Recipe::ingredients`] — the row carrying the
    /// quantity and unit.
    pub index: u32,
    /// The text actually written in the step, which is the alias when
    /// the recipe used one (`@garlic|it{}` reads "it").
    pub name: String,
    /// Byte offset of `name` within [`CookStep::text`]. Always on a
    /// char boundary: the name is spliced in whole.
    pub start: u32,
    /// Byte length of `name`.
    pub len: u32,
}

/// `Vec<CookStep>` newtype — JSON column.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct CookSteps(pub Vec<CookStep>);

impl CookSteps {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<CookStep>> for CookSteps {
    fn from(v: Vec<CookStep>) -> Self {
        Self(v)
    }
}

impl std::ops::Deref for CookSteps {
    type Target = Vec<CookStep>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Wire shape for a parsed `.cook` file. The original source is
/// preserved verbatim in `source` so editors can round-trip
/// without re-rendering through the cooklang printer.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "recipes", repo)]
pub struct Recipe {
    /// Vault-relative, forward-slash separated, e.g.
    /// `Cookbook/Truffle Pasta.cook` (wiki-relative).
    /// Identity — primary key.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,

    /// Display title. Pulled from `>> title:` metadata, or
    /// falls back to the filename stem.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// `>> description:` from metadata.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,

    /// `>> course:` from metadata. Free-form; canonical set in
    /// [`Course`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub course: Option<String>,

    /// `>> cuisine:` from metadata.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    #[architect(filterable)]
    pub cuisine: Option<String>,

    /// `>> prep time:` in whole minutes, when parseable.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "prepMinutes"
    )]
    pub prep_minutes: Option<u32>,

    /// `>> cook time:` in whole minutes.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "cookMinutes"
    )]
    pub cook_minutes: Option<u32>,

    /// `>> servings:` — base yield. Drives scaling at mealprep
    /// time.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub servings: Option<u32>,

    /// Ingredients extracted from `@name{qty%unit}` lines, in
    /// document order. Names are wikilink targets.
    #[serde(skip_serializing_if = "Ingredients::is_empty", default)]
    #[architect(json)]
    pub ingredients: Ingredients,

    /// Rendered step text in document order. Plain string per
    /// step. Authoring goes through [`Recipe::source`].
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub steps: StringList,

    /// Structured steps — same order/text as [`Recipe::steps`], but
    /// each step's timers (`~{…}`) extracted so a cook-along UI can
    /// offer one-tap countdowns. Derived from `source` at parse time.
    #[serde(
        skip_serializing_if = "CookSteps::is_empty",
        default,
        rename = "cookSteps"
    )]
    #[architect(json)]
    pub cook_steps: CookSteps,

    /// Cookware names from `#pan{}`.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub cookware: StringList,

    /// Sub-recipe references — paths from `@@./path/recipe{}`.
    #[serde(
        skip_serializing_if = "StringList::is_empty",
        default,
        rename = "nestedRecipes"
    )]
    #[architect(json)]
    pub nested_recipes: StringList,

    /// `>> tags:` (comma-separated in the metadata block).
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub tags: StringList,

    /// `>> source:` — URL, citation, or wikilink.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "sourceUrl")]
    pub source_url: Option<String>,

    /// File mtime when scanned.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    /// Raw cooklang source. The source of truth — editors
    /// mutate this and re-parse.
    pub source: String,

    /// Images sitting beside the recipe file, by naming convention.
    /// Filled in by the store when it reads from disk; the parser can't
    /// know about them because they aren't in the cooklang.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    #[architect(json)]
    pub images: RecipeImages,
}

/// Quantities scale unless cooklang says otherwise, so that's the
/// serde default and the case worth writing out is the pinned one.
fn scalable_default() -> bool {
    true
}

fn is_scalable(b: &bool) -> bool {
    *b
}

/// One ingredient line. `qty` is the numeric quantity for math;
/// `qty_display` keeps the original display form (ranges,
/// fractions, text). Held inline as JSON inside
/// [`Recipe::ingredients`].
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct Ingredient {
    /// Cooklang ingredient name. Wikilink target.
    pub name: String,

    /// Optional alias from `@flour|all-purpose flour{}`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub alias: Option<String>,

    /// Numeric quantity. `None` for `"to taste"` / text values.
    /// For a range (`{1-2%tbsp}`) this is the low end and
    /// [`Self::qty_max`] the high one.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub qty: Option<f64>,

    /// High end of a range quantity; `None` for a plain number.
    /// Kept so scaling a range produces a range — `1-2 tbsp` doubled
    /// is `2-4 tbsp`, not `3`.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "qtyMax")]
    pub qty_max: Option<f64>,

    /// Free-form unit string. Empty when no unit.
    #[serde(default)]
    pub unit: String,

    /// Whether this quantity moves when the recipe is scaled.
    ///
    /// Cooklang lets a quantity be pinned with `=` — `@salt{=1%tsp}`
    /// is one teaspoon whether you cook one portion or six, which is
    /// how you write "season the pan", not "season per head". Scaling
    /// a fixed quantity is a real mistake, so the flag has to survive
    /// the trip to whatever does the scaling.
    #[serde(default = "scalable_default", skip_serializing_if = "is_scalable")]
    pub scalable: bool,

    /// Original display form, including ranges / fractions /
    /// text. Use for rendering; use [`Self::qty`] for math.
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "qtyDisplay"
    )]
    pub qty_display: Option<String>,

    /// Cooklang note — `@butter{20%g}(softened)`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub note: Option<String>,

    /// `true` when the ingredient line carries `?`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,

    /// `true` when the line is a recipe reference (`@@...`).
    /// `name` then holds the recipe path.
    #[serde(
        default,
        skip_serializing_if = "std::ops::Not::not",
        rename = "isRecipeRef"
    )]
    pub is_recipe_ref: bool,
}

/// Canonical course values. Recipes round-trip arbitrary
/// strings; this is a hint for UI grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Course {
    Breakfast,
    Lunch,
    Dinner,
    Main,
    Side,
    Snack,
    Dessert,
    Drink,
}

impl Course {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Breakfast => "breakfast",
            Self::Lunch => "lunch",
            Self::Dinner => "dinner",
            Self::Main => "main",
            Self::Side => "side",
            Self::Snack => "snack",
            Self::Dessert => "dessert",
            Self::Drink => "drink",
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[must_use]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "breakfast" => Some(Self::Breakfast),
            "lunch" => Some(Self::Lunch),
            "dinner" => Some(Self::Dinner),
            "main" | "entree" | "main-course" => Some(Self::Main),
            "side" | "side-dish" => Some(Self::Side),
            "snack" => Some(Self::Snack),
            "dessert" | "sweet" => Some(Self::Dessert),
            "drink" | "beverage" => Some(Self::Drink),
            _ => None,
        }
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the vault-relative `path` (recipes have no UUID).
#[cfg(feature = "atom")]
impl architect::StoreEntity for Recipe {
    type Key = String;
    fn key(&self) -> String {
        self.path.clone()
    }
}
