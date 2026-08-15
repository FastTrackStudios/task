//! Cooklang source → [`Recipe`].
//!
//! Wraps `cooklang::CooklangParser` (all extensions enabled,
//! bundled units). Projects the parsed AST into our flat wire
//! shape: a list of ingredients with numeric quantities (when
//! possible), a list of rendered step strings, and metadata
//! lifted from the `>> key: value` block.

use chrono::{DateTime, Utc};
use cooklang::{Converter, CooklangParser, Extensions, Value};
use thiserror::Error;

use crate::model::{
    CookStep, CookSteps, Ingredient, Recipe, RecipeTimer, StepCookware, StepIngredient, StepLink,
    StringList,
};

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("cooklang parse failed: {0}")]
    Cooklang(String),
}

/// Parse a `.cook` source string into a [`Recipe`].
pub fn parse_cook(path: &str, source: &str) -> Result<Recipe, ParseError> {
    parse_cook_at(path, source, None)
}

/// Like [`parse_cook`] but stamps `date_modified` from the
/// caller (typically the file's mtime).
pub fn parse_cook_at(
    path: &str,
    source: &str,
    date_modified: Option<DateTime<Utc>>,
) -> Result<Recipe, ParseError> {
    let parser = parser();
    let (parsed, _report) = parser
        .parse(source)
        .into_result()
        .map_err(|e| ParseError::Cooklang(format!("{e:?}")))?;

    let name = parsed
        .metadata
        .title()
        .map_or_else(|| basename_of(path), str::to_string);
    let description = parsed.metadata.description().map(str::to_string);
    let course = take_meta_str(&parsed.metadata, "course");
    let cuisine = take_meta_str(&parsed.metadata, "cuisine");
    let tags = parsed
        .metadata
        .tags()
        .map(|ts| ts.into_iter().map(|s| s.into_owned()).collect())
        .unwrap_or_default();
    let source_url = parsed.metadata.source().and_then(|s| {
        s.url()
            .map(str::to_string)
            .or_else(|| s.name().map(str::to_string))
    });

    let (prep_minutes, cook_minutes) = match parsed.metadata.time(parser.converter()) {
        Some(cooklang::metadata::RecipeTime::Total(t)) => (None, Some(t)),
        Some(cooklang::metadata::RecipeTime::Composed {
            prep_time,
            cook_time,
        }) => (prep_time, cook_time),
        None => (None, None),
    };

    let servings = parsed.metadata.servings().and_then(|s| s.as_number());

    let cookware = parsed.cookware.iter().map(|c| c.name.clone()).collect();

    // cooklang indexes a step's ingredients against its own full list,
    // which includes rows we don't list (references, and repeat
    // mentions of something already introduced). Our `ingredients` drops
    // those, so a step's index has to be translated before it can point
    // at a row the reader can actually see.
    let mut listed_index: Vec<Option<u32>> = Vec::with_capacity(parsed.ingredients.len());
    let mut listed = 0u32;
    for i in &parsed.ingredients {
        if i.modifiers().should_be_listed() {
            listed_index.push(Some(listed));
            listed += 1;
        } else {
            listed_index.push(None);
        }
    }

    // Every `@@` reference, whether or not it carries a path. Cooklang
    // only builds a `reference` for a path-ish form like `@@./sauce`;
    // a bare `@@sauce` is still a recipe reference, just without one.
    // Collecting the name in that case lets the resolver find it by
    // stem instead of dropping the link on the floor.
    let mut nested_recipes: Vec<String> = parsed
        .ingredients
        .iter()
        .filter(|i| i.modifiers().contains(cooklang::Modifiers::RECIPE))
        .map(|i| {
            i.reference
                .as_ref()
                .map_or_else(|| i.name.clone(), |r| r.path("/"))
        })
        .collect();

    // …plus the vault's own link form, `[[Hot Honey]]{6}`. This is the
    // spelling to prefer: it is the same syntax every other note in the
    // vault uses to point at a page, so the cookbook participates in
    // the wiki graph instead of carrying a private path convention.
    // Cooklang passes `[[…]]` through as plain text, so each one also
    // becomes a synthetic recipe-ref ingredient — that is the shape a
    // `@@ref` already produces, and it lets fulfillment treat both
    // identically (see `mealplan::fulfillment::flatten`).
    let mut ingredients: Vec<Ingredient> = parsed
        .ingredients
        .iter()
        .filter(|i| i.modifiers().should_be_listed())
        .map(project_ingredient)
        .collect();
    for link in crate::wiki::scan_recipe_links(source) {
        if nested_recipes
            .iter()
            .any(|n| n.eq_ignore_ascii_case(&link.target))
        {
            continue;
        }
        ingredients.push(Ingredient {
            name: link.target.clone(),
            alias: None,
            qty: link.servings,
            qty_max: None,
            unit: String::new(),
            qty_display: link.servings.map(|q| format!("{q}")),
            scalable: true,
            note: None,
            optional: false,
            is_recipe_ref: true,
        });
        nested_recipes.push(link.target);
    }
    let nested_recipes: StringList = nested_recipes.into_iter().collect();

    // Structured steps: ingredient / cookware / timer names kept inline
    // (no more `·` placeholders) and timers extracted. `steps` is the
    // same text, kept for the existing index/grep/wiki consumers.
    // Each step carries the name of the `= Section` it sits under, so
    // cook mode can walk "Prep" and "Cook" as separate phases. An
    // unnamed section (or a recipe with no `=` headings at all) leaves
    // `section: None` — one anonymous run of steps, exactly as before.
    let mut pending_lead: Vec<(Option<String>, String)> = Vec::new();
    // A cooklang `> …` block is an aside, not an instruction. Fold each
    // one into the notes of the step it follows rather than emitting it
    // as a numbered step of its own — a warning about what you just did
    // reads as nonsense when presented as the next thing to do. A note
    // with no step before it in its section opens that section instead.
    let mut cook_steps: Vec<CookStep> = Vec::new();
    for section in &parsed.sections {
        let name = section.name.clone().filter(|n| !n.trim().is_empty());
        let first_of_section = cook_steps.len();
        for content in &section.content {
            match content {
                cooklang::Content::Step(step) => {
                    let mut projected = project_step(step, &parsed, &listed_index);
                    projected.section = name.clone();
                    cook_steps.push(projected);
                }
                cooklang::Content::Text(t) => {
                    let text = t.trim();
                    if text.is_empty() {
                        continue;
                    }
                    match cook_steps.len() > first_of_section {
                        true => cook_steps
                            .last_mut()
                            .expect("a step exists in this section")
                            .notes
                            .push(text.to_string()),
                        // Nothing to hang it on yet — carry it to the
                        // first step of the section as its lead-in.
                        false => pending_lead.push((name.clone(), text.to_string())),
                    }
                }
            }
        }
    }
    // Leading notes belong to the step that opens their section.
    for (section, note) in pending_lead {
        if let Some(step) = cook_steps.iter_mut().find(|s| s.section == section) {
            step.notes.insert(0, note);
        }
    }
    let steps: StringList = cook_steps.iter().map(|s| s.text.clone()).collect();

    Ok(Recipe {
        path: path.to_string(),
        name,
        description,
        course,
        cuisine,
        prep_minutes,
        cook_minutes,
        servings,
        ingredients: ingredients.into_iter().collect(),
        steps,
        cook_steps: CookSteps::from(cook_steps),
        cookware,
        nested_recipes,
        tags,
        source_url,
        date_modified,
        source: source.to_string(),
        // Found on disk by the store, not written in the cooklang.
        images: crate::model::RecipeImages::default(),
    })
}

fn project_ingredient(i: &cooklang::Ingredient) -> Ingredient {
    // `scalable` is cooklang's own answer to "does this move when the
    // recipe does" — `@salt{=1%tsp}` is pinned. Ranges keep both ends
    // so scaling `1-2 tbsp` gives `2-4`, not the midpoint doubled.
    let (qty, qty_max, unit, qty_display, scalable) = match &i.quantity {
        Some(q) => {
            let unit = q.unit().unwrap_or_default().to_string();
            let (qty, qty_max) = match q.value() {
                Value::Range { start, end } => (Some(start.value()), Some(end.value())),
                other => (number_value(other), None),
            };
            let display = Some(format!("{}", q.value()));
            (qty, qty_max, unit, display, q.scalable())
        }
        None => (None, None, String::new(), None, true),
    };
    Ingredient {
        name: i.name.clone(),
        alias: i.alias.clone(),
        qty,
        qty_max,
        unit,
        qty_display,
        scalable,
        note: i.note.clone(),
        optional: i.modifiers().contains(cooklang::Modifiers::OPT),
        is_recipe_ref: i.modifiers().contains(cooklang::Modifiers::RECIPE),
    }
}

fn number_value(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => Some(n.value()),
        Value::Range { start, end } => Some(f64::midpoint(f64::from(*start), f64::from(*end))),
        Value::Text(_) => None,
    }
}

/// Render one step to readable text, resolving each `Item`'s index into
/// the recipe-level component vecs so ingredient / cookware / timer
/// names land inline, and collecting the step's timers as structured
/// [`RecipeTimer`]s for one-tap countdowns.
fn project_step(
    step: &cooklang::Step,
    recipe: &cooklang::Recipe,
    listed_index: &[Option<u32>],
) -> CookStep {
    let mut text = String::new();
    let mut timers = Vec::new();
    let mut ingredients: Vec<StepIngredient> = Vec::new();
    let mut cookware: Vec<StepCookware> = Vec::new();
    for item in &step.items {
        match item {
            cooklang::Item::Text { value } => text.push_str(value),
            cooklang::Item::Ingredient { index } => {
                if let Some(ing) = recipe.ingredients.get(*index) {
                    let name = ing.alias.as_deref().unwrap_or(&ing.name);
                    // Span recorded before the splice, so it points at
                    // this occurrence and not a later identical one.
                    let start = text.len();
                    text.push_str(name);
                    if let Some(Some(row)) = listed_index.get(*index) {
                        ingredients.push(StepIngredient {
                            index: *row,
                            name: name.to_string(),
                            start: start as u32,
                            len: name.len() as u32,
                        });
                    }
                }
            }
            cooklang::Item::Cookware { index } => {
                if let Some(cw) = recipe.cookware.get(*index) {
                    let start = text.len();
                    text.push_str(&cw.name);
                    cookware.push(StepCookware {
                        index: *index as u32,
                        name: cw.name.clone(),
                        start: start as u32,
                        len: cw.name.len() as u32,
                    });
                }
            }
            cooklang::Item::Timer { index } => {
                if let Some(timer) = recipe.timers.get(*index) {
                    let projected = project_timer(timer);
                    text.push_str(&projected.display);
                    timers.push(projected);
                }
            }
            cooklang::Item::InlineQuantity { index } => {
                if let Some(q) = recipe.inline_quantities.get(*index) {
                    text.push_str(&q.to_string());
                }
            }
        }
    }
    // Trimming shifts every span left by whatever led the string.
    let lead = (text.len() - text.trim_start().len()) as u32;
    let text = text.trim().to_string();
    for si in &mut ingredients {
        si.start = si.start.saturating_sub(lead);
    }
    for cw in &mut cookware {
        cw.start = cw.start.saturating_sub(lead);
    }
    // Anything the trailing trim ate can no longer be pointed at.
    ingredients.retain(|si| (si.start + si.len) as usize <= text.len());
    cookware.retain(|cw| (cw.start + cw.len) as usize <= text.len());

    // Scanned against the finished text so the spans need no shifting:
    // cooklang emits `[[…]]` verbatim, so whatever is here is what the
    // reader will be asked to draw over.
    let links = crate::wiki::scan_links(&text)
        .into_iter()
        .map(|l| StepLink {
            target: l.target,
            display: l.display,
            is_recipe: l.is_recipe,
            start: l.start as u32,
            len: l.len as u32,
        })
        .collect();

    CookStep {
        text,
        timers,
        // Filled in by the caller, which knows the enclosing section.
        section: None,
        notes: Vec::new(),
        cookware,
        ingredients,
        links,
    }
}

/// A cooklang `~name{qty%unit}` timer → our [`RecipeTimer`]. Converts
/// the quantity to whole seconds (a bare/unknown unit is read as
/// minutes — the cooking default).
fn project_timer(t: &cooklang::Timer) -> RecipeTimer {
    let (seconds, display) = match &t.quantity {
        Some(q) => (timer_seconds(q), q.to_string()),
        None => (0, t.name.clone().unwrap_or_default()),
    };
    RecipeTimer {
        name: t.name.clone(),
        seconds,
        display,
    }
}

fn timer_seconds(q: &cooklang::Quantity) -> u32 {
    let Some(val) = number_value(q.value()) else {
        return 0;
    };
    let mult = match q.unit().map(|u| u.trim().to_ascii_lowercase()).as_deref() {
        Some("s" | "sec" | "secs" | "second" | "seconds") => 1.0,
        Some("h" | "hr" | "hrs" | "hour" | "hours") => 3600.0,
        // minutes, plus the unitless default
        _ => 60.0,
    };
    (val * mult).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

fn take_meta_str(m: &cooklang::Metadata, key: &str) -> Option<String> {
    m.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn basename_of(path: &str) -> String {
    std::path::Path::new(path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string()
}

fn parser() -> &'static CooklangParser {
    use std::sync::OnceLock;
    static P: OnceLock<CooklangParser> = OnceLock::new();
    P.get_or_init(|| CooklangParser::new(Extensions::all(), Converter::bundled()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_recipe() {
        let src = ">> title: Pasta\n>> servings: 2\n\nBoil @pasta{200%g} in salted water.";
        let r = parse_cook("Cookbook/Pasta.cook", src).expect("parse");
        assert_eq!(r.name, "Pasta");
        assert_eq!(r.servings, Some(2));
        assert_eq!(r.ingredients.len(), 1);
        assert_eq!(r.ingredients[0].name, "pasta");
        assert_eq!(r.ingredients[0].qty, Some(200.0));
        assert_eq!(r.ingredients[0].unit, "g");
        assert_eq!(r.steps.len(), 1);
        // Ingredient names are inlined into the step text now (no `·`).
        assert_eq!(r.steps[0], "Boil pasta in salted water.");
        assert!(!r.steps[0].contains('·'));
    }

    #[test]
    fn extracts_timers_and_inlines_names() {
        let src = "\
>> title: Steeped Tea

Boil @water{500%ml} in a #kettle, then steep the @tea bag{1} for ~steep{4%minutes}.

Rest the cup for ~{30%seconds} before sipping.";
        let r = parse_cook("Cookbook/Tea.cook", src).unwrap();
        assert_eq!(r.cook_steps.len(), 2);

        // Step 1: names inlined, one named timer = 4 minutes = 240s.
        let s1 = &r.cook_steps[0];
        assert!(s1.text.contains("Boil water"), "{}", s1.text);
        assert!(s1.text.contains("kettle"), "{}", s1.text);
        assert_eq!(s1.timers.len(), 1);
        assert_eq!(s1.timers[0].name.as_deref(), Some("steep"));
        assert_eq!(s1.timers[0].seconds, 240);

        // Step 2: a bare timer in seconds.
        let s2 = &r.cook_steps[1];
        assert_eq!(s2.timers.len(), 1);
        assert_eq!(s2.timers[0].name, None);
        assert_eq!(s2.timers[0].seconds, 30);

        // `steps` mirrors `cook_steps` text.
        assert_eq!(r.steps.len(), r.cook_steps.len());
        assert_eq!(r.steps[0], r.cook_steps[0].text);
    }

    #[test]
    fn falls_back_to_filename_for_title() {
        let r = parse_cook("Cookbook/Truffle Pasta.cook", "Just cook it.").unwrap();
        assert_eq!(r.name, "Truffle Pasta");
    }

    #[test]
    fn parses_metadata_block() {
        let src = "\
>> title: Carbonara
>> description: Roman classic
>> course: dinner
>> cuisine: italian
>> servings: 4
>> prep time: 5 min
>> cook time: 15 min
>> tags: weeknight, pasta

Cook the @pasta{400%g}.
";
        let r = parse_cook("Cookbook/Carbonara.cook", src).unwrap();
        assert_eq!(r.name, "Carbonara");
        assert_eq!(r.description.as_deref(), Some("Roman classic"));
        assert_eq!(r.course.as_deref(), Some("dinner"));
        assert_eq!(r.cuisine.as_deref(), Some("italian"));
        assert_eq!(r.servings, Some(4));
        assert_eq!(r.prep_minutes, Some(5));
        assert_eq!(r.cook_minutes, Some(15));
        assert_eq!(r.tags.0, vec!["weeknight", "pasta"]);
    }

    #[test]
    fn optional_ingredient_modifier() {
        let r = parse_cook("Cookbook/X.cook", "Top with @?parmesan{}.").unwrap();
        assert_eq!(r.ingredients.len(), 1);
        assert!(r.ingredients[0].optional);
    }

    #[test]
    fn recipe_reference_is_collected() {
        let r = parse_cook(
            "Cookbook/Pizza.cook",
            "Make @@./Shared/Pizza Dough{}, then top.",
        )
        .unwrap();
        assert!(!r.nested_recipes.is_empty());
    }

    #[test]
    fn a_note_is_an_aside_not_a_step() {
        // `>` blocks are warnings and asides. Numbering one as a step
        // tells the cook to *do* something that already happened.
        let src = ">> title: X\n\nFry the @garlic{2%clove}.\n\n> Don't brown it — it turns bitter.\n\nAdd @stock{200%ml}.";
        let r = parse_cook("Cookbook/X.cook", src).unwrap();
        assert_eq!(r.cook_steps.len(), 2, "two instructions, not three");
        assert_eq!(
            r.cook_steps[0].notes,
            vec!["Don't brown it — it turns bitter."]
        );
        assert!(
            r.cook_steps[1].notes.is_empty(),
            "the note hangs off the step it followed"
        );
    }

    #[test]
    fn a_note_opening_a_section_leads_its_first_step() {
        let src =
            ">> title: X\n\n= Cook\n\n> Get everything to hand first.\n\nFry the @garlic{2%clove}.";
        let r = parse_cook("Cookbook/X.cook", src).unwrap();
        assert_eq!(r.cook_steps.len(), 1);
        assert_eq!(r.cook_steps[0].notes, vec!["Get everything to hand first."]);
    }

    #[test]
    fn steps_point_at_the_cookware_they_use() {
        let src = ">> title: X\n\nWarm the @oil{1%tbsp} in a #wide pan{}.";
        let r = parse_cook("Cookbook/X.cook", src).unwrap();
        let step = &r.cook_steps[0];
        assert_eq!(step.cookware.len(), 1);
        let cw = &step.cookware[0];
        assert_eq!(cw.name, "wide pan");
        // The span must slice back out exactly, so the mention can be
        // marked in place and linked to the equipment list.
        let start = cw.start as usize;
        assert_eq!(&step.text[start..start + cw.len as usize], "wide pan");
        assert_eq!(r.cookware[cw.index as usize], "wide pan");
    }

    #[test]
    fn steps_point_at_the_ingredients_they_use() {
        let src =
            ">> title: X\n\nFry @garlic{4%clove} in @olive oil{3%tbsp}.\n\nAdd @salt{1%pinch}.";
        let r = parse_cook("Cookbook/X.cook", src).unwrap();

        let first = &r.cook_steps[0];
        let names: Vec<&str> = first.ingredients.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["garlic", "olive oil"]);

        // Every span must slice back out of the step text exactly —
        // that is what lets a reader highlight the word in place.
        for si in first.ingredients.iter() {
            let start = si.start as usize;
            let end = start + si.len as usize;
            assert_eq!(
                &first.text[start..end],
                si.name,
                "span mismatch in {:?}",
                first.text
            );
        }

        // And the index must land on the row carrying the quantity.
        let garlic = &r.ingredients[first.ingredients[0].index as usize];
        assert_eq!(garlic.name, "garlic");
        assert_eq!(garlic.qty, Some(4.0));

        let second = &r.cook_steps[1];
        assert_eq!(second.ingredients.len(), 1);
        let salt = &r.ingredients[second.ingredients[0].index as usize];
        assert_eq!(salt.name, "salt");
    }

    #[test]
    fn step_spans_survive_a_leading_trim() {
        let r = parse_cook(
            "Cookbook/X.cook",
            ">> title: X\n\n  Add @flour{200%g} slowly.",
        )
        .unwrap();
        let step = &r.cook_steps[0];
        let si = &step.ingredients[0];
        let start = si.start as usize;
        assert_eq!(&step.text[start..start + si.len as usize], "flour");
    }

    #[test]
    fn steps_carry_their_section_name() {
        let src = "\
>> title: Sectioned

= Prep

Chop @onion{1}.

= Cook

Fry the onion for ~{5%min}.

Season and serve.
";
        let r = parse_cook("Cookbook/Sectioned.cook", src).unwrap();
        let sections: Vec<_> = r.cook_steps.iter().map(|s| s.section.as_deref()).collect();
        assert_eq!(
            sections,
            vec![Some("Prep"), Some("Cook"), Some("Cook")],
            "each step should report the `= heading` it sits under"
        );
    }

    #[test]
    fn unsectioned_recipe_leaves_section_none() {
        let r = parse_cook("Cookbook/Flat.cook", "Boil @pasta{200%g}.\nDrain it.").unwrap();
        assert!(
            r.cook_steps.iter().all(|s| s.section.is_none()),
            "a recipe with no `=` headings stays one anonymous run of steps"
        );
    }
}

#[cfg(test)]
mod migration_check {
    use super::*;
    #[test]
    fn parses_multiword_ingredients_from_migration() {
        // Output shape produced by migrate-md-to-cook.
        let src = "\
>> title: Truffle Pasta
>> servings: 2

Cook @Pasta{200%g} for 8 minutes.
Drain and toss with @Olive Oil{30%ml} and @Truffles{5%g}.
";
        let r = parse_cook("Cookbook/Truffle Pasta.cook", src).unwrap();
        assert_eq!(
            r.ingredients.len(),
            3,
            "got {:?}",
            r.ingredients.iter().map(|i| &i.name).collect::<Vec<_>>()
        );
        assert!(
            r.ingredients
                .iter()
                .any(|i| i.name.eq_ignore_ascii_case("olive oil")),
            "olive oil missing"
        );
    }
}
