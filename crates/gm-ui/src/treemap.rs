//! The layout of the hotspot map (docs/guide/code-health.md, "Hotspot map"): the squarified
//! treemap of Bruls, Huizing and van Wijk, in plain geometry. The files are grouped by the folder
//! they lie in under a scope; each folder is a box with a header strip, and its files are boxes
//! inside it. No GPUI types here: the view converts the rectangles once.

use std::collections::BTreeMap;

/// The most files the map lays out: the largest by size, ties by path.
pub(crate) const MAP_FILES: usize = 2000;

/// A file to lay out: its index in the report, its path and its bytes.
type Entry<'a> = (usize, &'a str, u64);

/// A rectangle in the map's own units (pixels in the view).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

impl Rect {
    fn short_side(self) -> f64 {
        self.w.min(self.h)
    }

    /// Whether the rectangle has an area and its shorter side is at least `min_side`.
    fn fits(self, min_side: f64) -> bool {
        let short = self.short_side();
        short > 0.0 && short >= min_side
    }
}

/// A folder of the map: its full path (`""` for the files directly in the scope), its box and
/// the files drawn inside it, each with its index in the report.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MapNode {
    pub(crate) folder: String,
    pub(crate) rect: Rect,
    pub(crate) files: Vec<(usize, Rect)>,
}

/// Tiles `area` with one rectangle per size, in the same order. `sizes` are expected sorted
/// largest first, which keeps the boxes square. Each rectangle's area is its share of the total;
/// a size of zero or less gets an empty rectangle.
pub(crate) fn squarify(sizes: &[f64], area: Rect) -> Vec<Rect> {
    let mut rects = vec![Rect::default(); sizes.len()];
    let total: f64 = sizes.iter().filter(|size| **size > 0.0).sum();
    if total <= 0.0 || area.w <= 0.0 || area.h <= 0.0 {
        return rects;
    }
    let scale = area.w * area.h / total;
    let areas: Vec<f64> = sizes
        .iter()
        .map(|&size| if size > 0.0 { size * scale } else { 0.0 })
        .collect();
    let mut rest = area;
    let mut row: Vec<usize> = Vec::new();
    for index in (0..areas.len()).filter(|&index| areas[index] > 0.0) {
        let side = rest.short_side();
        let row_areas: Vec<f64> = row.iter().map(|&placed| areas[placed]).collect();
        let before = worst(&row_areas, side);
        let mut extended = row_areas;
        extended.push(areas[index]);
        if worst(&extended, side) <= before {
            row.push(index);
        } else {
            rest = lay_row(&row, &areas, rest, &mut rects);
            row = vec![index];
        }
    }
    if !row.is_empty() {
        lay_row(&row, &areas, rest, &mut rects);
    }
    rects
}

/// The worst aspect ratio of a row of `areas` laid along a side of length `side`. An empty row
/// has no ratio yet, so any row beats it.
fn worst(areas: &[f64], side: f64) -> f64 {
    let total: f64 = areas.iter().sum();
    let largest = areas.iter().copied().fold(0.0, f64::max);
    let smallest = areas.iter().copied().fold(f64::INFINITY, f64::min);
    if areas.is_empty() || total <= 0.0 || smallest <= 0.0 {
        return f64::INFINITY;
    }
    let side_squared = side * side;
    let total_squared = total * total;
    (side_squared * largest / total_squared).max(total_squared / (side_squared * smallest))
}

/// Lays `row` along the shorter side of `rest` and returns what is left of `rest`.
fn lay_row(row: &[usize], areas: &[f64], rest: Rect, rects: &mut [Rect]) -> Rect {
    if rest.w <= 0.0 || rest.h <= 0.0 {
        return rest;
    }
    let total: f64 = row.iter().map(|&placed| areas[placed]).sum();
    if rest.w >= rest.h {
        // A column along the left edge, as wide as the row's area over the height.
        let width = total / rest.h;
        let mut top = rest.y;
        for &placed in row {
            let height = areas[placed] / width;
            rects[placed] = Rect {
                x: rest.x,
                y: top,
                w: width,
                h: height,
            };
            top += height;
        }
        Rect {
            x: rest.x + width,
            y: rest.y,
            w: (rest.w - width).max(0.0),
            h: rest.h,
        }
    } else {
        // A row along the top edge, as tall as the row's area over the width.
        let height = total / rest.w;
        let mut left = rest.x;
        for &placed in row {
            let width = areas[placed] / height;
            rects[placed] = Rect {
                x: left,
                y: rest.y,
                w: width,
                h: height,
            };
            left += width;
        }
        Rect {
            x: rest.x,
            y: rest.y + height,
            w: rest.w,
            h: (rest.h - height).max(0.0),
        }
    }
}

/// Lays out the files under `scope` in `area`. Each entry is `(index in the report, path,
/// bytes)`. Files outside the scope and files of zero bytes are left out, and only the
/// `MAP_FILES` largest files (ties by path) are laid out, whatever the caller passes.
///
/// The folders under the scope are squarified in `area`; inside each folder, below a strip of
/// `header` units for its name, its files are squarified. A file or folder whose shorter side is
/// under `min_side` is dropped, and so is a folder too short for its header. A file directly in
/// the scope belongs to the folder `""`. Each area is a size in bytes, clamped to 4 GiB, which
/// only matters for a file that large.
pub(crate) fn layout(
    files: &[(usize, &str, u64)],
    scope: &str,
    area: Rect,
    header: f64,
    min_side: f64,
) -> Vec<MapNode> {
    let mut under: Vec<(usize, &str, u64)> = files
        .iter()
        .copied()
        .filter(|&(_, path, bytes)| bytes > 0 && relative(path, scope).is_some())
        .collect();
    under.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(b.1)));
    under.truncate(MAP_FILES);

    let mut groups: BTreeMap<&str, Vec<Entry>> = BTreeMap::new();
    for &(index, path, bytes) in &under {
        let rest = relative(path, scope).unwrap_or_default();
        let folder = rest.split_once('/').map_or("", |(first, _)| first);
        groups.entry(folder).or_default().push((index, path, bytes));
    }
    let mut folders: Vec<(&str, f64, Vec<Entry>)> = groups
        .into_iter()
        .map(|(name, files)| {
            let size = files.iter().map(|file| area_of(file.2)).sum();
            (name, size, files)
        })
        .collect();
    folders.sort_by(|a, b| b.1.total_cmp(&a.1));

    let sizes: Vec<f64> = folders.iter().map(|folder| folder.1).collect();
    let mut nodes = Vec::new();
    for ((name, _, files), rect) in folders.iter().zip(squarify(&sizes, area)) {
        if rect.h <= header || !rect.fits(min_side) {
            continue;
        }
        let body = Rect {
            x: rect.x,
            y: rect.y + header,
            w: rect.w,
            h: rect.h - header,
        };
        let file_sizes: Vec<f64> = files.iter().map(|file| area_of(file.2)).collect();
        let placed = squarify(&file_sizes, body)
            .into_iter()
            .zip(files)
            .filter(|(rect, _)| rect.fits(min_side))
            .map(|(rect, file)| (file.0, rect))
            .collect();
        nodes.push(MapNode {
            folder: folder_path(scope, name),
            rect,
            files: placed,
        });
    }
    nodes
}

/// The path of `path` below `scope`, or `None` when it is not under it. Empty scope: the path.
fn relative<'a>(path: &'a str, scope: &str) -> Option<&'a str> {
    if scope.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(scope)?.strip_prefix('/')
    }
}

/// The full path of the folder `name` under `scope`; empty for the files directly in the scope.
fn folder_path(scope: &str, name: &str) -> String {
    match (name.is_empty(), scope.is_empty()) {
        (true, _) => String::new(),
        (false, true) => name.to_owned(),
        (false, false) => format!("{scope}/{name}"),
    }
}

/// A file's share of the map, from its size: the byte count, clamped to `u32::MAX`.
fn area_of(bytes: u64) -> f64 {
    f64::from(u32::try_from(bytes).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 10.0,
        y: 20.0,
        w: 600.0,
        h: 400.0,
    };

    fn contains(outer: Rect, inner: Rect) -> bool {
        let eps = 1e-6;
        inner.x >= outer.x - eps
            && inner.y >= outer.y - eps
            && inner.x + inner.w <= outer.x + outer.w + eps
            && inner.y + inner.h <= outer.y + outer.h + eps
    }

    fn area_sum(rects: &[Rect]) -> f64 {
        rects.iter().map(|rect| rect.w * rect.h).sum()
    }

    #[test]
    fn the_rectangles_tile_the_area() {
        let sizes = [6.0, 6.0, 4.0, 3.0, 2.0, 2.0, 1.0];
        let rects = squarify(&sizes, AREA);
        assert!((area_sum(&rects) - AREA.w * AREA.h).abs() < 1e-6);
        assert!(rects.iter().all(|&rect| contains(AREA, rect)));
    }

    #[test]
    fn the_order_and_the_count_are_kept_and_zero_sizes_are_empty() {
        let sizes = [5.0, 0.0, 3.0, -2.0, 1.0];
        let rects = squarify(&sizes, AREA);
        assert_eq!(rects.len(), sizes.len());
        assert_eq!(rects[1], Rect::default());
        assert_eq!(rects[3], Rect::default());
        // The rectangle of a larger size is larger.
        assert!(rects[0].w * rects[0].h > rects[2].w * rects[2].h);
        assert!(rects[2].w * rects[2].h > rects[4].w * rects[4].h);
    }

    #[test]
    fn a_single_item_fills_the_area() {
        let rects = squarify(&[7.0], AREA);
        assert_eq!(rects.len(), 1);
        assert!(contains(AREA, rects[0]));
        assert!((area_sum(&rects) - AREA.w * AREA.h).abs() < 1e-6);
    }

    #[test]
    fn nothing_to_lay_out_gives_empty_rectangles() {
        assert_eq!(squarify(&[], AREA), Vec::<Rect>::new());
        assert_eq!(squarify(&[0.0, 0.0], AREA), vec![Rect::default(); 2]);
        assert_eq!(squarify(&[1.0], Rect::default()), vec![Rect::default()]);
    }

    #[test]
    fn files_are_grouped_by_the_first_segment_below_the_scope() {
        let files = [
            (0, "src/a/x.rs", 300),
            (1, "src/a/deep/y.rs", 100),
            (2, "src/b.rs", 200),
            (3, "src/c/z.rs", 50),
        ];
        let nodes = layout(&files, "src", AREA, 0.0, 0.0);
        let folders: Vec<&str> = nodes.iter().map(|node| node.folder.as_str()).collect();
        // Largest first: src/a (400 bytes), the files directly in src (200), src/c (50).
        assert_eq!(folders, ["src/a", "", "src/c"]);
        let a = &nodes[0];
        let indices: Vec<usize> = a.files.iter().map(|file| file.0).collect();
        assert_eq!(indices, [0, 1]);
        assert_eq!(nodes[1].files.len(), 1);
        assert_eq!(nodes[1].files[0].0, 2);
        assert_eq!(nodes[2].files[0].0, 3);
    }

    #[test]
    fn at_the_root_a_folder_is_its_first_segment() {
        let files = [
            (0, "src/a.rs", 10),
            (1, "docs/b/c.md", 10),
            (2, "README", 5),
        ];
        let nodes = layout(&files, "", AREA, 0.0, 0.0);
        let mut folders: Vec<&str> = nodes.iter().map(|node| node.folder.as_str()).collect();
        folders.sort_unstable();
        assert_eq!(folders, ["", "docs", "src"]);
    }

    #[test]
    fn files_outside_the_scope_and_empty_files_are_left_out() {
        let files = [
            (0, "src/a.rs", 10),
            (1, "srcx/b.rs", 10),
            (2, "src/empty.rs", 0),
        ];
        let nodes = layout(&files, "src", AREA, 0.0, 0.0);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].files.len(), 1);
        assert_eq!(nodes[0].files[0].0, 0);
    }

    #[test]
    fn boxes_sit_inside_their_parents_below_the_header() {
        let files = [(0, "a/x.rs", 500), (1, "a/y.rs", 300), (2, "b/z.rs", 200)];
        let nodes = layout(&files, "", AREA, 20.0, 0.0);
        for node in &nodes {
            assert!(contains(AREA, node.rect));
            let body = Rect {
                x: node.rect.x,
                y: node.rect.y + 20.0,
                w: node.rect.w,
                h: node.rect.h - 20.0,
            };
            for (_, rect) in &node.files {
                assert!(contains(body, *rect));
            }
            let files_area: f64 = node.files.iter().map(|(_, rect)| rect.w * rect.h).sum();
            assert!((files_area - body.w * body.h).abs() < 1e-6);
        }
    }

    #[test]
    fn tiny_files_and_folders_too_short_for_a_header_are_dropped() {
        let files = [(0, "big/a.rs", 1_000_000), (1, "big/tiny.rs", 1)];
        let nodes = layout(&files, "", AREA, 10.0, 0.0);
        assert_eq!(nodes[0].files.len(), 2);

        let nodes = layout(&files, "", AREA, 10.0, 100.0);
        assert_eq!(nodes[0].files.len(), 1);
        assert_eq!(nodes[0].files[0].0, 0);

        let thin = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 12.0,
        };
        assert_eq!(layout(&files, "", thin, 10.0, 0.0)[0].files.len(), 2);
        assert!(layout(&files, "", thin, 12.0, 0.0).is_empty());
    }

    #[test]
    fn at_most_the_largest_2000_files_are_laid_out() {
        let owned: Vec<(usize, String, u64)> = (0..2100)
            .map(|index| (index, format!("d/f{index:05}.rs"), 1 + index as u64))
            .collect();
        let files: Vec<(usize, &str, u64)> = owned
            .iter()
            .map(|(index, path, bytes)| (*index, path.as_str(), *bytes))
            .collect();
        let nodes = layout(&files, "", AREA, 0.0, 0.0);
        let placed: Vec<usize> = nodes
            .iter()
            .flat_map(|node| node.files.iter().map(|file| file.0))
            .collect();
        assert_eq!(placed.len(), MAP_FILES);
        assert!(placed.iter().all(|&index| index >= 100));
    }

    #[test]
    fn the_same_input_gives_the_same_output() {
        let files = [
            (0, "a/x.rs", 5),
            (1, "b/y.rs", 5),
            (2, "c.rs", 5),
            (3, "a/z.rs", 9),
        ];
        assert_eq!(
            layout(&files, "", AREA, 12.0, 2.0),
            layout(&files, "", AREA, 12.0, 2.0)
        );
        assert_eq!(
            squarify(&[3.0, 3.0, 2.0], AREA),
            squarify(&[3.0, 3.0, 2.0], AREA)
        );
    }
}
