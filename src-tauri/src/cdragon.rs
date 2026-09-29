//! Position art from Community Dragon, for the one kind of art Data Dragon
//! does not publish. DEVELOPMENT.md §5.3.
//!
//! ## One source per kind of art
//!
//! Community Dragon was a source once before, for summoner spells, as a
//! *second* place to find art Data Dragon already had, and it was taken out:
//! two sources for one picture is two ways for a row to draw the wrong one
//! (`ddragon::spell_art_map`). This is the other case. Data Dragon has no
//! position icons at all, so there is nothing for this to disagree with, and
//! nothing here falls back to anything else. A position icon comes from here
//! or it is not drawn.
//!
//! ## Pinned to Data Dragon's patch, not `latest`
//!
//! Community Dragon is an unedited extract of the client, run by the
//! community, with no promise that a path survives the next client
//! restructure. `latest` is also PBE-adjacent and moves under us. So the
//! directory asked for is the patch Data Dragon already resolved (`16.19.1`
//! → `16.19`), which makes a broken path show up at a patch boundary, cached
//! per patch like everything else, instead of at random.
//!
//! ## Only the lit part is kept
//!
//! The client's icon is two layers: a dimmed map at half opacity, and the lit
//! lane on top of it. The row draws the badge at about twelve pixels, where
//! the dimmed layer antialiases into a grey smudge, and the frontend uses the
//! file as a CSS *mask* so the theme decides its colour. The file is rewritten
//! once on the way into the cache to hold just the `class="active"` shapes, so
//! the UI never parses or inserts markup it fetched.

use std::path::{Path, PathBuf};

const CDN: &str = "https://raw.communitydragon.org";

/// Where the client keeps its position art, under a patch directory.
const POSITION_PATH: &str = "plugins/rcp-fe-lol-static-assets/global/default/svg";

/// The file a role's icon is filed under. The five words are the ones
/// DEVELOPMENT.md §3.1 names; the client calls support `utility`.
fn position_file(role: &str) -> Option<&'static str> {
    Some(match role {
        "Top" => "position-top.svg",
        "Jungle" => "position-jungle.svg",
        "Middle" => "position-middle.svg",
        "Bottom" => "position-bottom.svg",
        "Support" => "position-utility.svg",
        _ => return None,
    })
}

/// Community Dragon's patch directory for a Data Dragon version: the first
/// two parts. `None` for anything that is not a number-dot-number, so a
/// version Riot spelled differently asks for nothing rather than for a
/// directory that does not exist.
fn patch(ddragon_version: &str) -> Option<String> {
    let mut parts = ddragon_version.split('.');
    let (major, minor) = (parts.next()?, parts.next()?);
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (numeric(major) && numeric(minor)).then(|| format!("{major}.{minor}"))
}

/// The client's icon reduced to its lit shapes: a fresh `<svg>` carrying only
/// the original's `viewBox`, and the basic shapes marked `class="active"`.
/// `None` if there is no `viewBox` or nothing is lit, because an icon that is
/// all dimmed map would be the wrong picture and a missing one is not.
///
/// Deliberately not an XML parser. The files are a handful of self-closing
/// shapes each, and anything this does not recognise is dropped rather than
/// passed through, so a restructured file costs the badge and nothing more.
fn lit_only(svg: &str) -> Option<String> {
    let start = svg.find("<svg")?;
    let open = &svg[start..start + svg[start..].find('>')? + 1];
    let view_box = attribute(open, "viewBox")?;

    let mut shapes = Vec::new();
    let mut rest = &svg[start + open.len()..];
    while let Some(at) = rest.find('<') {
        let Some(len) = rest[at..].find('>') else { break };
        let tag = &rest[at..at + len + 1];
        let name = tag[1..].split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if SHAPES.contains(&name) && tag.ends_with("/>") && tag.contains(r#"class="active""#) {
            shapes.push(tag);
        }
        rest = &rest[at + len + 1..];
    }
    if shapes.is_empty() {
        return None;
    }
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{view_box}">{}</svg>"#,
        shapes.join("")
    ) + "\n")
}

/// The elements a position icon is drawn with. Anything else is dropped.
const SHAPES: [&str; 6] = ["path", "polygon", "polyline", "rect", "circle", "ellipse"];

/// A double-quoted attribute's value, if it is made of nothing a `viewBox`
/// cannot contain.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let at = tag.find(&format!(r#" {name}=""#))? + name.len() + 3;
    let value = &tag[at..at + tag[at..].find('"')?];
    value
        .bytes()
        .all(|b| b.is_ascii_digit() || b" .-,".contains(&b))
        .then_some(value)
}

/// The cached, lit-only icon for a role, fetched the first time it is asked
/// for. `dir` is this module's own cache directory; `ddragon_version` is the
/// one `ddragon` resolved, which picks the patch.
///
/// `None` for everything that could go wrong, the same contract as the rest
/// of the art: the row simply has no badge.
pub async fn position_icon(dir: &Path, ddragon_version: &str, role: &str) -> Option<PathBuf> {
    let file = position_file(role)?;
    let patch = patch(ddragon_version)?;
    let path = dir.join(&patch).join("position").join(file);
    if path.is_file() {
        return Some(path);
    }

    let body = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .ok()?
        .get(format!("{CDN}/{patch}/{POSITION_PATH}/{file}"))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;

    let Some(lit) = lit_only(&body) else {
        crate::warn!("cdragon", "{file} on {patch} had no lit shapes");
        return None;
    };
    std::fs::create_dir_all(path.parent()?).ok()?;
    std::fs::write(&path, lit).ok()?;
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The five roles, and the client's own name for support.
    #[test]
    fn every_role_names_a_file_and_nothing_else_does() {
        assert_eq!(position_file("Support"), Some("position-utility.svg"));
        for role in ["Top", "Jungle", "Middle", "Bottom"] {
            assert_eq!(
                position_file(role),
                Some(format!("position-{}.svg", role.to_lowercase()).as_str())
            );
        }
        for other in ["", "top", "Arena", "Utility", "NONE"] {
            assert_eq!(position_file(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_patch_is_the_first_two_parts_of_the_ddragon_version() {
        assert_eq!(patch("16.19.1").as_deref(), Some("16.19"));
        assert_eq!(patch("15.1.1").as_deref(), Some("15.1"));
        assert_eq!(patch("16.19").as_deref(), Some("16.19"));
        for bad in ["", "16", "lolpatch_7.20", "16.x.1", "../16.19"] {
            assert_eq!(patch(bad), None, "{bad:?}");
        }
    }

    /// The real middle-lane file, as the client ships it on 16.19.
    const MIDDLE: &str = r##"<?xml-stylesheet type="text/css" href="./glow.css"?>
<svg xmlns="http://www.w3.org/2000/svg" width="34" height="34" viewBox="0 0 34 34">
  <path opacity="0.5" fill="#785a28" fill-rule="evenodd" d="M30,12.968l-4.008,4L26,26H17l-4,4H30ZM16.979,8L21,4H4V20.977L8,17,8,8h8.981Z"/>
  <polygon class="active" fill="#c8aa6e" points="25 4 4 25 4 30 9 30 30 9 30 4 25 4"/>
</svg>
"##;

    /// The dimmed map is what smudges at twelve pixels, and the stylesheet
    /// reference points at a file that is not cached beside it.
    #[test]
    fn only_the_lit_shapes_survive() {
        let lit = lit_only(MIDDLE).unwrap();
        assert!(
            lit.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 34 34">"#),
            "{lit}"
        );
        assert!(lit.contains(r#"<polygon class="active""#), "{lit}");
        assert!(!lit.contains("opacity"), "the dimmed layer survived: {lit}");
        assert!(!lit.contains("xml-stylesheet"), "{lit}");
        assert!(lit.trim_end().ends_with("</svg>"), "{lit}");
    }

    /// Jungle and support are a single lit path with no map under them.
    #[test]
    fn a_file_that_is_all_lit_keeps_everything_it_draws() {
        let jungle = r#"<svg viewBox="0 0 34 34"><path class="active" fill-rule="evenodd" d="M25,3Z"/></svg>"#;
        assert_eq!(
            lit_only(jungle).as_deref(),
            Some(concat!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 34 34">"#,
                r#"<path class="active" fill-rule="evenodd" d="M25,3Z"/></svg>"#,
                "\n"
            ))
        );
    }

    /// A restructured file costs the badge, never draws the wrong thing, and
    /// never passes anything through that it does not recognise.
    #[test]
    fn a_file_with_nothing_lit_is_no_icon() {
        assert_eq!(lit_only(""), None);
        assert_eq!(lit_only("<html>404</html>"), None);
        assert_eq!(lit_only(r#"<svg viewBox="0 0 34 34"/>"#), None);
        assert_eq!(
            lit_only(r#"<svg><path class="active" d="M0Z"/></svg>"#),
            None,
            "no viewBox, so nothing to draw it in"
        );
        assert_eq!(
            lit_only(r#"<svg viewBox="0 0 34 34"><path opacity="0.5" d="M0Z"/></svg>"#),
            None,
            "the dimmed map alone is not a picture of the role"
        );
    }

    /// Nothing but the viewBox and lit shapes is carried across, whatever
    /// else the file holds.
    #[test]
    fn nothing_that_is_not_a_lit_shape_is_passed_through() {
        let hostile = concat!(
            r#"<svg viewBox="0 0 34 34" onload="alert(1)">"#,
            r#"<script class="active"/><script>alert(2)</script>"#,
            r#"<image class="active" href="https://example.com/x.png"/>"#,
            r#"<path class="active" d="M0Z"/></svg>"#,
        );
        let lit = lit_only(hostile).unwrap();
        for gone in ["onload", "script", "image", "example.com"] {
            assert!(!lit.contains(gone), "{gone} survived: {lit}");
        }
        assert!(lit.contains(r#"<path class="active" d="M0Z"/>"#), "{lit}");
        assert_eq!(lit_only(r#"<svg viewBox="0 0 34 34&quot;"><path class="active" d="M0Z"/></svg>"#), None);
    }
}
