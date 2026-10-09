// Which UI image registry keys a descriptor names: a tree's `background`, each
// `Image` asset, and each 9-slice border texture. A pure walk; which keys load
// when is app policy.
// See: context/lib/ui.md §5

use super::{AnchoredTree, Widget};

impl AnchoredTree {
    /// Every UI image key this tree names, its background first, then the
    /// root's keys in tree order. Duplicates are kept.
    pub fn image_keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        if let Some(background) = &self.background {
            keys.push(background.image.as_str());
        }
        collect_image_keys(&self.root, &mut keys);
        keys
    }
}

impl Widget {
    /// Every UI image key this widget and its descendants name, in tree order.
    /// Duplicates are kept. A `Glyph` names none: its art is resolved per frame
    /// from the input glyph directories, not from `uiImages`.
    pub fn image_keys(&self) -> Vec<&str> {
        let mut keys = Vec::new();
        collect_image_keys(self, &mut keys);
        keys
    }
}

fn collect_image_keys<'a>(widget: &'a Widget, keys: &mut Vec<&'a str>) {
    // No `_` arm: a new widget kind must decide whether it names an image.
    match widget {
        Widget::Image(image) => keys.push(image.asset.as_str()),
        Widget::Panel(panel) => {
            if let Some(border) = &panel.border {
                keys.push(border.texture.as_str());
            }
        }
        Widget::VStack(container) | Widget::HStack(container) => {
            if let Some(border) = &container.border {
                keys.push(border.texture.as_str());
            }
            for child in &container.children {
                collect_image_keys(child, keys);
            }
        }
        Widget::Grid(grid) => {
            for child in &grid.children {
                collect_image_keys(child, keys);
            }
        }
        Widget::Text(_)
        | Widget::Spacer(_)
        | Widget::Button(_)
        | Widget::Slider(_)
        | Widget::Bar(_)
        | Widget::Ring(_)
        | Widget::Announce(_)
        | Widget::Glyph(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bottom-anchored tree with a background, a panel border, an image
    /// nested in a grid inside a bordered stack, and a glyph.
    const TREE_JSON: &str = r#"{
        "anchor": "bottom", "offset": [0.0, 0.0],
        "background": { "image": "shots/e1m1" },
        "root": { "kind": "vstack", "gap": 0.0, "padding": 0.0, "align": "start",
            "border": { "texture": "ui/frame", "slice": [4.0, 4.0, 4.0, 4.0], "tint": [1.0, 1.0, 1.0, 1.0] },
            "children": [
                { "kind": "panel", "fill": [0.0, 0.0, 0.0, 1.0],
                  "border": { "texture": "ui/panel", "slice": [2.0, 2.0, 2.0, 2.0], "tint": [1.0, 1.0, 1.0, 1.0] } },
                { "kind": "grid", "gap": 0.0, "padding": 0.0, "align": "start", "cols": 1,
                  "children": [ { "kind": "image", "asset": "ui/icon" } ] },
                { "kind": "glyph", "command": "jump" },
                { "kind": "text", "content": "Loading", "fontSize": 16.0, "color": [1.0, 1.0, 1.0, 1.0] }
            ] }
    }"#;

    #[test]
    fn image_keys_name_background_borders_and_nested_image_assets_in_tree_order() {
        let tree: AnchoredTree = serde_json::from_str(TREE_JSON).expect("fixture parses");
        assert_eq!(
            tree.image_keys(),
            ["shots/e1m1", "ui/frame", "ui/panel", "ui/icon"],
            "a glyph and text name no image"
        );
        assert_eq!(
            tree.root.image_keys(),
            ["ui/frame", "ui/panel", "ui/icon"],
            "a widget walk has no background"
        );
    }

    #[test]
    fn image_keys_of_a_tree_without_images_are_empty() {
        let tree: AnchoredTree = serde_json::from_str(
            r#"{ "anchor": "center", "offset": [0.0, 0.0],
                 "root": { "kind": "spacer", "flexGrow": 1.0 } }"#,
        )
        .expect("fixture parses");
        assert!(tree.image_keys().is_empty());
    }
}
