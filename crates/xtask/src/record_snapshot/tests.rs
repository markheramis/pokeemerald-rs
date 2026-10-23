use std::path::PathBuf;

use assets::pack::AssetPack;

use super::{
    capture_loaded, default_paths, fnv1a64, git_sha, render_meta, run_with_paths,
    RecordSnapshotError, TITLE_FRAME_INDEX,
};
use crate::Scene;

const WINDOW_FRAME_GREEN_BGR555: u16 = 0x03E0;
const MAIN_MENU_BACKGROUND_BGR555: u16 = 0x4104;
const REPLACEMENT_BACKGROUND_BGR555: u16 = 0x001F;
/// [`MAIN_MENU_BACKGROUND_BGR555`] expanded by hand, independent of
/// `rendering`: `0x4104` holds 5-bit channels r=4, g=8, b=16, and
/// `to_rgb888`'s `(c << 3) | (c >> 2)` gives 33, 66, 132. Kept as literals
/// so a channel-order regression cannot be pasted over from observed output.
const MAIN_MENU_BACKGROUND_RGB888: [u8; 3] = [33, 66, 132];

#[derive(Clone, Copy)]
enum EntryKind {
    Image,
    Palette,
}

impl EntryKind {
    fn tag(self) -> u8 {
        match self {
            Self::Image => 0,
            Self::Palette => 1,
        }
    }
}

struct Entry {
    id: &'static str,
    kind: EntryKind,
    meta: Vec<u8>,
    payload: Vec<u8>,
}

fn write_synthetic_pack(mut entries: Vec<Entry>) -> Vec<u8> {
    entries.sort_by(|left, right| left.id.cmp(right.id));

    let header_size =
        assets::pack::MAGIC.len() + std::mem::size_of::<u32>() + std::mem::size_of::<u32>();
    let mut directory_size = 0usize;
    for entry in &entries {
        directory_size += std::mem::size_of::<u16>()
            + entry.id.len()
            + std::mem::size_of::<u8>()
            + std::mem::size_of::<u64>()
            + std::mem::size_of::<u64>()
            + entry.meta.len();
    }
    let mut offset = header_size + directory_size;
    let mut offsets = Vec::new();
    for entry in &entries {
        offsets.push(offset);
        offset += entry.payload.len();
    }

    let mut pack = Vec::new();
    pack.extend_from_slice(&assets::pack::MAGIC);
    pack.extend_from_slice(&assets::pack::FORMAT_VERSION.to_le_bytes());
    pack.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_le_bytes());
    for (entry, &offset) in entries.iter().zip(&offsets) {
        pack.extend_from_slice(&u16::try_from(entry.id.len()).unwrap().to_le_bytes());
        pack.extend_from_slice(entry.id.as_bytes());
        pack.push(entry.kind.tag());
        pack.extend_from_slice(&u64::try_from(offset).unwrap().to_le_bytes());
        pack.extend_from_slice(&u64::try_from(entry.payload.len()).unwrap().to_le_bytes());
        pack.extend_from_slice(&entry.meta);
    }
    for entry in &entries {
        pack.extend_from_slice(&entry.payload);
    }
    pack
}

fn image_meta(width: u32, height: u32, bit_depth: u8) -> Vec<u8> {
    let mut meta = Vec::new();
    meta.extend_from_slice(&width.to_le_bytes());
    meta.extend_from_slice(&height.to_le_bytes());
    meta.push(bit_depth);
    meta
}

fn palette_meta(color_count: u16) -> Vec<u8> {
    color_count.to_le_bytes().to_vec()
}

fn synthetic_main_menu_pack_bytes() -> Vec<u8> {
    synthetic_main_menu_pack_bytes_with_background(MAIN_MENU_BACKGROUND_BGR555)
}

fn synthetic_main_menu_pack_bytes_with_background(background: u16) -> Vec<u8> {
    let frame_pixels = vec![1u8; 24 * 24];
    let mut frame_palette = vec![0u8; 32];
    frame_palette[2..4].copy_from_slice(&WINDOW_FRAME_GREEN_BGR555.to_le_bytes());

    let font_pixels =
        vec![0u8; (assets::fonts::SHEET_WIDTH * assets::fonts::SHEET_HEIGHT) as usize];

    let mut bg_palette = vec![0u8; 32];
    bg_palette[0..2].copy_from_slice(&background.to_le_bytes());

    write_synthetic_pack(vec![
        Entry {
            id: "text-window/image/1",
            kind: EntryKind::Image,
            meta: image_meta(24, 24, 4),
            payload: frame_pixels,
        },
        Entry {
            id: "text-window/palette/1",
            kind: EntryKind::Palette,
            meta: palette_meta(16),
            payload: frame_palette,
        },
        Entry {
            id: "font/normal/glyphs",
            kind: EntryKind::Image,
            meta: image_meta(assets::fonts::SHEET_WIDTH, assets::fonts::SHEET_HEIGHT, 2),
            payload: font_pixels,
        },
        Entry {
            id: "interface/palette/main_menu_bg",
            kind: EntryKind::Palette,
            meta: palette_meta(16),
            payload: bg_palette,
        },
    ])
}

fn scratch_path(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "pokeemerald-rs-xtask-record-snapshot-test-{}-{:?}-{name}",
        std::process::id(),
        std::thread::current().id()
    ))
}

struct ScratchGuard(PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_pack(name: &str) -> (PathBuf, ScratchGuard) {
    let path = scratch_path(&format!("{name}.pack"));
    std::fs::write(&path, synthetic_main_menu_pack_bytes()).unwrap();
    let guard = ScratchGuard(path.clone());
    (path, guard)
}

/// Fails unless every staged pointer file left in `output_dir` is named by
/// `error` as a retained path: the retention policy reports what it keeps.
#[cfg(unix)]
fn assert_staged_pointers_reported(output_dir: &std::path::Path, error: &str) {
    for entry in std::fs::read_dir(output_dir).unwrap() {
        let path = entry.unwrap().path();
        if path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains(".tmp.")
        {
            assert!(
                error.contains(&format!("last known path: {}", path.display())),
                "staged pointer {} was left behind unreported: {error}",
                path.display()
            );
        }
    }
}

fn visible_generation(output_dir: &std::path::Path, scene: Scene) -> Option<PathBuf> {
    let pointer = output_dir.join(format!("{}.generation", scene.name()));
    std::fs::read_to_string(pointer)
        .ok()
        .map(|generation| output_dir.join(generation.trim()))
}

#[test]
fn fnv1a64_is_deterministic_and_input_sensitive() {
    let a = fnv1a64(b"hello world");
    let b = fnv1a64(b"hello world");
    let c = fnv1a64(b"hello worlD");
    assert_eq!(a, b, "same input must hash identically every time");
    assert_ne!(c, a, "different input must (in practice) hash differently");
}

#[test]
fn fnv1a64_matches_the_published_offset_basis_for_empty_input() {
    assert_eq!(fnv1a64(&[]), 0xcbf2_9ce4_8422_2325);
}

#[test]
fn fnv1a64_matches_the_published_non_empty_known_answer() {
    assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
}

#[test]
fn git_sha_of_this_checkout_is_full_length_with_an_optional_dirty_suffix() {
    let sha = git_sha(&crate::extract::repo_root()).expect("this checkout is a real git repo");
    let hex = sha.strip_suffix("-dirty").unwrap_or(&sha);
    assert_eq!(hex.len(), 40, "a git SHA-1 is 40 hex characters: {sha}");
    assert!(
        hex.chars().all(|c| c.is_ascii_hexdigit()),
        "a git SHA must be all hex digits: {sha}"
    );
}

#[test]
fn git_sha_marks_a_dirty_worktree_and_leaves_a_clean_one_bare() {
    let repo = scratch_path("dirty-marker-repo");
    let _guard = ScratchGuard(repo.clone());
    std::fs::create_dir_all(&repo).unwrap();

    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .expect("git must be on PATH")
            .status
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "--quiet"]);
    git(&["config", "user.email", "test@example.invalid"]);
    git(&["config", "user.name", "test"]);
    git(&["config", "commit.gpgsign", "false"]);
    std::fs::write(repo.join("tracked.txt"), b"one").unwrap();
    git(&["add", "tracked.txt"]);
    git(&["commit", "--quiet", "-m", "initial"]);

    let clean = git_sha(&repo).expect("a freshly committed repo has a HEAD");
    assert!(
        !clean.ends_with("-dirty"),
        "a clean worktree must report a bare SHA: {clean}"
    );

    std::fs::write(repo.join("tracked.txt"), b"two").unwrap();
    let dirty = git_sha(&repo).expect("HEAD is unchanged by an uncommitted edit");
    assert_eq!(
        dirty,
        format!("{clean}-dirty"),
        "an uncommitted change must mark the SHA dirty"
    );
}

#[test]
fn git_sha_is_none_for_a_path_with_no_git_repo() {
    let scratch = scratch_path("not-a-repo");
    std::fs::create_dir_all(&scratch).unwrap();
    let guard = ScratchGuard(scratch.clone());
    // A dangling .git file prevents discovery of an enclosing repository.
    std::fs::write(
        scratch.join(".git"),
        b"gitdir: this-path-deliberately-does-not-exist\n",
    )
    .unwrap();
    assert!(git_sha(&scratch).is_none());
    drop(guard);
}

#[test]
fn render_meta_has_the_documented_fixed_field_order() {
    let text = render_meta(
        Scene::Title,
        &[],
        "cafef00ddeadbeef",
        "deadbeefcafef00d",
        "abc123",
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines,
        vec![
            "scene: title",
            "width: 240",
            "height: 160",
            "pixel_format: rgb888",
            "inputs: none",
            "rgb_hash: fnv1a64:cafef00ddeadbeef",
            "pack_hash: fnv1a64:deadbeefcafef00d",
            "git_sha: abc123",
        ]
    );
}

#[test]
fn render_meta_joins_multiple_inputs_with_commas() {
    let text = render_meta(Scene::MainMenuOption, &["DPAD_DOWN", "A"], "1", "0", "x");
    assert!(text.contains("inputs: DPAD_DOWN,A\n"));
}

#[test]
fn missing_pack_fails_closed_with_pack_error() {
    let pack_path = scratch_path("missing.pack");
    let output_dir = scratch_path("missing-out");
    let err = run_with_paths(Scene::MainMenuNewGame, &pack_path, &output_dir).unwrap_err();
    assert!(matches!(err, RecordSnapshotError::Pack(_)));
    assert!(
        !output_dir.exists(),
        "a failed capture must not create output"
    );
}

#[test]
fn title_scene_against_a_main_menu_only_pack_fails_closed_with_scene_error() {
    let (pack_path, _guard) = write_pack("title-against-main-menu-pack");
    let output_dir = scratch_path("title-scene-error-out");
    let err = run_with_paths(Scene::Title, &pack_path, &output_dir).unwrap_err();
    assert!(matches!(err, RecordSnapshotError::Scene(_)));
}

#[test]
fn the_title_capture_frame_has_press_start_visible() {
    assert!(
        pokeemerald_rs::title::press_start_visible(TITLE_FRAME_INDEX),
        "TITLE_FRAME_INDEX must witness the Press Start banner"
    );
}

#[test]
fn main_menu_new_game_writes_rgb_channels_in_order_and_correct_metadata() {
    let (pack_path, _pack_guard) = write_pack("main-menu-new-game-report");
    let output_dir = scratch_path("main-menu-new-game-out");
    let out_guard = ScratchGuard(output_dir.clone());

    let report = run_with_paths(Scene::MainMenuNewGame, &pack_path, &output_dir).unwrap();

    assert_eq!(report.payload_len, 240 * 160 * 3);
    let rgb_bytes = std::fs::read(&report.rgb_path).unwrap();
    assert_eq!(rgb_bytes.len(), report.payload_len);

    assert_eq!(
        &rgb_bytes[..3],
        &MAIN_MENU_BACKGROUND_RGB888,
        "the first pixel must preserve RGB channel order"
    );

    let meta = std::fs::read_to_string(&report.meta_path).unwrap();
    assert!(meta.contains("scene: main-menu-new-game\n"));
    assert!(meta.contains("width: 240\n"));
    assert!(meta.contains("height: 160\n"));
    assert!(meta.contains("pixel_format: rgb888\n"));
    assert!(meta.contains("inputs: none\n"));
    assert!(meta.contains(&format!("rgb_hash: fnv1a64:{}\n", report.rgb_hash)));
    assert!(meta.contains(&format!("pack_hash: fnv1a64:{}\n", report.pack_hash)));
    assert!(meta.contains(&format!("git_sha: {}\n", report.git_sha)));

    assert_eq!(report.rgb_hash, format!("{:016x}", fnv1a64(&rgb_bytes)));
    let pack_bytes = std::fs::read(&pack_path).unwrap();
    assert_eq!(report.pack_hash, format!("{:016x}", fnv1a64(&pack_bytes)));

    let visible = visible_generation(&output_dir, Scene::MainMenuNewGame).unwrap();
    assert_eq!(report.rgb_path.parent(), Some(visible.as_path()));
    assert_eq!(report.meta_path.parent(), Some(visible.as_path()));

    let leftovers: Vec<PathBuf> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "staging temporaries must be renamed away, not left behind: {leftovers:?}"
    );

    drop(out_guard);
}

#[test]
fn replacing_the_pack_path_cannot_change_loaded_pack_provenance() {
    let (pack_path, _pack_guard) = write_pack("loaded-pack-provenance");
    let original_bytes = std::fs::read(&pack_path).unwrap();
    let pack = AssetPack::load(&pack_path).unwrap();

    let replacement = synthetic_main_menu_pack_bytes_with_background(REPLACEMENT_BACKGROUND_BGR555);
    assert_ne!(replacement, original_bytes);
    std::fs::write(&pack_path, &replacement).unwrap();

    let output_dir = scratch_path("loaded-pack-provenance-out");
    let out_guard = ScratchGuard(output_dir.clone());
    let report = capture_loaded(Scene::MainMenuNewGame, &pack, &output_dir, || Ok(())).unwrap();
    let rgb = std::fs::read(&report.rgb_path).unwrap();

    assert_eq!(
        report.pack_hash,
        format!("{:016x}", fnv1a64(&original_bytes))
    );
    assert_ne!(report.pack_hash, format!("{:016x}", fnv1a64(&replacement)));
    assert_eq!(
        &rgb[..3],
        &MAIN_MENU_BACKGROUND_RGB888,
        "composition and provenance must both use the retained original buffer"
    );

    drop(out_guard);
}

#[test]
fn failure_after_rgb_staging_preserves_the_visible_generation() {
    let (pack_path, _pack_guard) = write_pack("failed-generation-commit");
    let pack = AssetPack::load(&pack_path).unwrap();
    let output_dir = scratch_path("failed-generation-commit-out");
    let out_guard = ScratchGuard(output_dir.clone());

    let injected_failure = || {
        Err(RecordSnapshotError::Write(
            output_dir.join("injected-after-rgb"),
            "injected failure".to_owned(),
        ))
    };
    let err =
        capture_loaded(Scene::MainMenuNewGame, &pack, &output_dir, injected_failure).unwrap_err();
    assert!(matches!(err, RecordSnapshotError::Write(_, _)), "{err}");
    assert!(
        visible_generation(&output_dir, Scene::MainMenuNewGame).is_none(),
        "an initial failure before the commit point must publish no generation"
    );

    let previous = capture_loaded(Scene::MainMenuNewGame, &pack, &output_dir, || Ok(())).unwrap();
    let pointer_path = output_dir.join("main-menu-new-game.generation");
    let previous_pointer = std::fs::read(&pointer_path).unwrap();
    let previous_rgb = std::fs::read(&previous.rgb_path).unwrap();
    let previous_meta = std::fs::read(&previous.meta_path).unwrap();

    let err = capture_loaded(Scene::MainMenuNewGame, &pack, &output_dir, || {
        Err(RecordSnapshotError::Write(
            output_dir.join("injected-after-rgb-again"),
            "injected failure".to_owned(),
        ))
    })
    .unwrap_err();
    assert!(matches!(err, RecordSnapshotError::Write(_, _)), "{err}");
    assert_eq!(std::fs::read(&pointer_path).unwrap(), previous_pointer);
    assert_eq!(std::fs::read(&previous.rgb_path).unwrap(), previous_rgb);
    assert_eq!(std::fs::read(&previous.meta_path).unwrap(), previous_meta);
    let retained: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".staged")
        })
        .collect();
    assert_eq!(
        retained.len(),
        2,
        "both failed captures retain their staged payloads"
    );
    assert!(retained
        .iter()
        .all(|path| path.join("main-menu-new-game.rgb").is_file()));
    assert!(retained
        .iter()
        .any(|path| err.to_string().contains(&path.display().to_string())));

    drop(out_guard);
}

#[test]
fn main_menu_option_records_its_dpad_down_input_and_a_different_frame() {
    let (pack_path, _pack_guard) = write_pack("main-menu-option-vs-new-game");

    let new_game_dir = scratch_path("main-menu-option-vs-new-game-out-a");
    let new_game_guard = ScratchGuard(new_game_dir.clone());
    let new_game = run_with_paths(Scene::MainMenuNewGame, &pack_path, &new_game_dir).unwrap();

    let option_dir = scratch_path("main-menu-option-vs-new-game-out-b");
    let option_guard = ScratchGuard(option_dir.clone());
    let option = run_with_paths(Scene::MainMenuOption, &pack_path, &option_dir).unwrap();

    let option_meta = std::fs::read_to_string(&option.meta_path).unwrap();
    assert!(option_meta.contains("inputs: DPAD_DOWN\n"));

    let new_game_rgb = std::fs::read(&new_game.rgb_path).unwrap();
    let option_rgb = std::fs::read(&option.rgb_path).unwrap();
    assert_ne!(
        new_game_rgb, option_rgb,
        "moving the selection must change the captured frame"
    );
    assert_ne!(
        new_game.rgb_hash, option.rgb_hash,
        "a different captured frame must hash differently"
    );

    drop(new_game_guard);
    drop(option_guard);
}

#[test]
fn capturing_the_same_scene_twice_is_byte_identical() {
    let (pack_path, _pack_guard) = write_pack("determinism");

    let dir_a = scratch_path("determinism-out-a");
    let guard_a = ScratchGuard(dir_a.clone());
    let first = run_with_paths(Scene::MainMenuNewGame, &pack_path, &dir_a).unwrap();

    let dir_b = scratch_path("determinism-out-b");
    let guard_b = ScratchGuard(dir_b.clone());
    let second = run_with_paths(Scene::MainMenuNewGame, &pack_path, &dir_b).unwrap();

    let rgb_a = std::fs::read(&first.rgb_path).unwrap();
    let rgb_b = std::fs::read(&second.rgb_path).unwrap();
    assert_eq!(
        rgb_a, rgb_b,
        ".rgb payload must be byte-identical across runs"
    );

    let meta_a = std::fs::read_to_string(&first.meta_path).unwrap();
    let meta_b = std::fs::read_to_string(&second.meta_path).unwrap();
    assert_eq!(meta_a, meta_b, ".meta must be byte-identical across runs");

    assert_eq!(first.rgb_hash, second.rgb_hash);
    assert_eq!(first.pack_hash, second.pack_hash);
    assert_eq!(first.git_sha, second.git_sha);

    drop(guard_a);
    drop(guard_b);
}

#[test]
fn default_paths_are_derived_without_capturing_into_the_repository() {
    let repo_root = crate::extract::repo_root();
    let expected_pack = repo_root.join(crate::extract::OUTPUT_RELATIVE_PATH);
    let expected_dir = repo_root.join("snapshots");
    let (pack_path, output_dir) = default_paths(&repo_root);
    assert_eq!(pack_path, expected_pack);
    assert_eq!(output_dir, expected_dir);
}

#[test]
#[ignore = "needs a local pack: run `cargo xtask extract` first"]
fn real_pack_scene_round_trips_the_capture_and_matches_a_second_run() {
    // Extraction rewrites the shared real pack non-atomically.
    let _pack = crate::extract::REAL_PACK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let output_dir = std::env::temp_dir().join(format!(
        "pokeemerald-rs-xtask-record-snapshot-real-pack-{}",
        std::process::id()
    ));
    let guard = ScratchGuard(output_dir.clone());

    let first = run_with_paths(
        Scene::MainMenuNewGame,
        &assets::pack::AssetPack::repo_pack_path(),
        &output_dir,
    )
    .expect("run `cargo xtask extract` first");
    let rgb_a = std::fs::read(&first.rgb_path).unwrap();
    assert!(
        rgb_a.iter().any(|&b| b != 0),
        "a real capture must not be entirely black"
    );

    let second = run_with_paths(
        Scene::MainMenuNewGame,
        &assets::pack::AssetPack::repo_pack_path(),
        &output_dir,
    )
    .unwrap();
    let rgb_b = std::fs::read(&second.rgb_path).unwrap();
    assert_eq!(
        rgb_a, rgb_b,
        "two captures of the same real pack must match byte-for-byte"
    );
    assert_eq!(first.rgb_hash, second.rgb_hash);
    assert_eq!(first.pack_hash, second.pack_hash);

    drop(guard);
}

/// The staging suffix value used to deterministically pin one pointer
/// candidate, matching `extract::publish`'s `TEST_STAGING_VALUE` convention.
const TEST_POINTER_STAGING_VALUE: u64 = 0x00AB_CDEF_0123;

/// A symlink planted at the first pointer-staging candidate is refused, not
/// followed; publication proceeds at the next unpredictable candidate.
#[cfg(unix)]
#[test]
fn a_planted_pointer_symlink_is_never_written_through() {
    let output_dir = scratch_path("pointer-symlink-out");
    let out_guard = ScratchGuard(output_dir.clone());
    let bystander_dir = scratch_path("pointer-symlink-bystander");
    let bystander_guard = ScratchGuard(bystander_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&bystander_dir).unwrap();

    let pointer_path = output_dir.join(format!("{}.generation", Scene::MainMenuNewGame.name()));
    let occupied = crate::staging_names::path_with_value(&pointer_path, TEST_POINTER_STAGING_VALUE);
    let free = crate::staging_names::path_with_value(&pointer_path, TEST_POINTER_STAGING_VALUE + 1);
    let bystander = bystander_dir.join("bystander");
    std::os::unix::fs::symlink(&bystander, &occupied).unwrap();

    let staged =
        super::stage_pointer_with_candidates(b"a-generation\n", [occupied.clone(), free.clone()])
            .unwrap();
    staged.publish(&pointer_path).unwrap();

    assert!(
        !bystander.exists(),
        "publishing followed the planted symlink and wrote outside {}",
        output_dir.display()
    );
    assert!(
        std::fs::symlink_metadata(&occupied)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a refused planted symlink must be left alone, not consumed as staging"
    );
    assert!(
        !free.exists(),
        "the free candidate is consumed and renamed onto the pointer, not left behind"
    );
    assert!(
        !std::fs::symlink_metadata(&pointer_path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the published pointer must be a regular file, not a planted symlink"
    );
    assert_eq!(std::fs::read(&pointer_path).unwrap(), b"a-generation\n");

    drop(bystander_guard);
    drop(out_guard);
}

/// A name a different owner already holds is left untouched; the bounded
/// walk retries the next unpredictable candidate instead of failing outright.
#[test]
fn a_colliding_first_pointer_candidate_is_left_untouched_in_favor_of_the_next_free_name() {
    let output_dir = scratch_path("pointer-collision-retry-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let pointer_path = output_dir.join(format!("{}.generation", Scene::MainMenuNewGame.name()));
    let occupied = crate::staging_names::path_with_value(&pointer_path, TEST_POINTER_STAGING_VALUE);
    let free = crate::staging_names::path_with_value(&pointer_path, TEST_POINTER_STAGING_VALUE + 1);
    std::fs::write(&occupied, b"someone else's staging file").unwrap();

    let staged =
        super::stage_pointer_with_candidates(b"a-generation\n", [occupied.clone(), free.clone()])
            .unwrap();
    staged.publish(&pointer_path).unwrap();

    assert_eq!(
        std::fs::read(&occupied).unwrap(),
        b"someone else's staging file",
        "a name already taken must be left to its owner, not overwritten"
    );
    assert!(
        !free.exists(),
        "the free candidate is consumed and renamed onto the pointer, not left behind"
    );
    assert_eq!(std::fs::read(&pointer_path).unwrap(), b"a-generation\n");

    drop(out_guard);
}

/// A pointer publication that fails after the generation was promoted leaves
/// that generation in place and names it, so the operator knows what to
/// remove; a directory in the pointer's slot is the failure driven here.
#[test]
fn a_failed_pointer_publication_names_the_generation_it_leaves_behind() {
    let scene = Scene::MainMenuNewGame;
    let output_dir = scratch_path("pointer-slot-taken-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir(output_dir.join(format!("{}.generation", scene.name()))).unwrap();

    let error =
        super::publish_generation(scene, &output_dir, b"rgb-bytes", b"meta-bytes", || Ok(()))
            .unwrap_err();

    let retained: Vec<PathBuf> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().contains("generation-"))
        })
        .collect();
    assert_eq!(
        retained.len(),
        1,
        "exactly one promoted generation is left: {retained:?}"
    );
    assert!(
        error
            .to_string()
            .contains(&retained[0].display().to_string()),
        "the error must name the retained generation {}: {error}",
        retained[0].display()
    );
    drop(out_guard);
}

/// Publication must stage the pointer outside every name the generation makes
/// guessable: links planted at all of them neither starve the capture nor take
/// a write.
#[cfg(unix)]
#[test]
fn publication_stages_the_pointer_outside_every_generation_derived_name() {
    /// Covers every generation-derived pointer name this process could reach.
    const PREDICTABLE_INDICES: u64 = 256;

    let output_dir = scratch_path("pointer-predictable-name-out");
    let out_guard = ScratchGuard(output_dir.clone());
    let bystander_dir = scratch_path("pointer-predictable-name-bystander");
    let bystander_guard = ScratchGuard(bystander_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&bystander_dir).unwrap();

    let scene = Scene::MainMenuNewGame;
    for index in 0..PREDICTABLE_INDICES {
        let generation = format!("{}.generation-{}-{index}", scene.name(), std::process::id());
        std::os::unix::fs::symlink(
            bystander_dir.join(format!("bystander-{index}")),
            output_dir.join(format!(".{generation}.pointer")),
        )
        .unwrap();
    }

    let (rgb_path, meta_path) =
        super::publish_generation(scene, &output_dir, b"rgb-bytes", b"meta-bytes", || Ok(()))
            .unwrap();

    assert_eq!(std::fs::read(&rgb_path).unwrap(), b"rgb-bytes");
    assert_eq!(std::fs::read(&meta_path).unwrap(), b"meta-bytes");
    let published = rgb_path.parent().unwrap().file_name().unwrap();
    let pointer_path = output_dir.join(format!("{}.generation", scene.name()));
    assert_eq!(
        std::fs::read(&pointer_path).unwrap(),
        format!("{}\n", published.to_str().unwrap()).as_bytes(),
        "the pointer must name the generation that was just published"
    );
    let escaped: Vec<_> = std::fs::read_dir(&bystander_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        escaped.is_empty(),
        "publishing followed a planted symlink and wrote {escaped:?} outside {}",
        output_dir.display()
    );

    drop(bystander_guard);
    drop(out_guard);
}

/// A promoting rename that fails must never turn into an unlink of whatever
/// now holds the staging name: a held-file identity check and a pathname
/// deletion are separate lookups, so a failed publish retains the staging
/// file on every platform and reports its last known path.
#[test]
fn a_failed_publish_retains_and_reports_its_staging_file() {
    let dir = scratch_path("failed-publish");
    let _guard = ScratchGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let staging_path = dir.join(".pointer.tmp");
    let unreachable_dest = dir.join("missing-parent").join("pointer");

    let staged = super::staging::stage(&staging_path, b"generation\n").unwrap();
    let error = staged.publish(&unreachable_dest).unwrap_err();

    assert_eq!(
        std::fs::read(&staging_path).unwrap(),
        b"generation\n",
        "a failed publish must retain the staging file, not remove it: {error}"
    );
    assert!(
        error
            .to_string()
            .contains(&format!("last known path: {}", staging_path.display())),
        "the error must report the retained staging file's path: {error}"
    );
}

/// A failed promoting rename leaves the staging pathname alone: a
/// replacement landing there right after the failure survives, and the
/// reported path still names it. The retained original is pinned by
/// `a_failed_publish_retains_and_reports_its_staging_file`.
#[cfg(unix)]
#[test]
fn a_failed_publish_leaves_a_replacement_planted_at_the_old_cleanup_boundary() {
    let dir = scratch_path("failed-publish-boundary");
    let _guard = ScratchGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let staging_path = dir.join(".pointer.tmp");
    let carried_off = dir.join("carried-off");
    let unreachable_dest = dir.join("missing-parent").join("pointer");

    let staged = super::staging::stage(&staging_path, b"generation\n").unwrap();
    let error = staged
        .publish_with(
            &unreachable_dest,
            || {},
            || {
                // The promoting rename has already failed by the time this runs.
                std::fs::rename(&staging_path, &carried_off).unwrap();
                std::fs::write(&staging_path, b"someone else's file").unwrap();
            },
        )
        .unwrap_err();

    assert_eq!(
        std::fs::read(&staging_path).unwrap(),
        b"someone else's file",
        "a failed publish removed a replacement it never owned: {error}"
    );
    assert_eq!(std::fs::read(&carried_off).unwrap(), b"generation\n");
    assert!(
        error
            .to_string()
            .contains(&format!("last known path: {}", staging_path.display())),
        "the error must still report the staging path even though a replacement now sits there: {error}"
    );
}

/// A replacement that wins the gap between `publish`'s ownership check and
/// its promoting rename is renamed onto the pointer path exactly as the
/// genuine staged file would have been; publication must still refuse it
/// rather than report success for whatever landed there.
#[cfg(unix)]
#[test]
fn a_replacement_racing_the_promoting_rename_is_promoted_but_never_accepted() {
    let dir = scratch_path("publish-race-foreign-file");
    let _guard = ScratchGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let staging_path = dir.join(".pointer.tmp");
    let carried_off = dir.join("carried-off");
    let pointer_path = dir.join("scene.generation");

    let staged = super::staging::stage(&staging_path, b"a-generation\n").unwrap();
    let error = staged
        .publish_with(
            &pointer_path,
            || {
                // Fires after the ownership check passes and before the
                // promoting rename: the genuine staged file is carried off
                // and a foreign regular file takes its name in the gap.
                std::fs::rename(&staging_path, &carried_off).unwrap();
                std::fs::write(&staging_path, b"attacker-controlled\n").unwrap();
            },
            || panic!("the promoting rename must succeed for this race to be exercised"),
        )
        .unwrap_err();

    assert_eq!(
        std::fs::read(&pointer_path).unwrap(),
        b"attacker-controlled\n",
        "the replacement is promoted onto the pointer path by the rename itself"
    );
    assert_eq!(
        std::fs::read(&carried_off).unwrap(),
        b"a-generation\n",
        "the genuine staged file is untouched at its carried-off path"
    );
    assert!(
        error
            .to_string()
            .contains(&format!("last known path: {}", pointer_path.display())),
        "the error must name the promoted pointer path: {error}"
    );
}

/// The same race, but the replacement is a symlink rather than a foreign
/// regular file: a promoted symlink must be refused just as a foreign
/// regular file is, and left in place for inspection.
#[cfg(unix)]
#[test]
fn a_symlink_racing_the_promoting_rename_is_promoted_but_never_accepted() {
    let dir = scratch_path("publish-race-symlink");
    let _guard = ScratchGuard(dir.clone());
    let bystander_dir = scratch_path("publish-race-symlink-bystander");
    let bystander_guard = ScratchGuard(bystander_dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&bystander_dir).unwrap();
    let staging_path = dir.join(".pointer.tmp");
    let carried_off = dir.join("carried-off");
    let pointer_path = dir.join("scene.generation");
    let bystander = bystander_dir.join("bystander");
    std::fs::write(&bystander, b"outside the output directory\n").unwrap();

    let staged = super::staging::stage(&staging_path, b"a-generation\n").unwrap();
    let error = staged
        .publish_with(
            &pointer_path,
            || {
                std::fs::rename(&staging_path, &carried_off).unwrap();
                std::os::unix::fs::symlink(&bystander, &staging_path).unwrap();
            },
            || panic!("the promoting rename must succeed for this race to be exercised"),
        )
        .unwrap_err();

    assert!(
        std::fs::symlink_metadata(&pointer_path)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the planted symlink is promoted onto the pointer path by the rename itself"
    );
    assert_eq!(std::fs::read(&carried_off).unwrap(), b"a-generation\n");
    assert!(
        error
            .to_string()
            .contains(&format!("last known path: {}", pointer_path.display())),
        "the error must name the promoted pointer path: {error}"
    );

    drop(bystander_guard);
}

/// Failure cleanup must only remove what this publish created. The generation
/// name is derived from the scene, the process id, and a counter, so another
/// writer with access to `output_dir` can take that name after the `exists`
/// probe. The promoting rename then fails, and the cleanup that follows must
/// not recursively delete a directory this publish never owned.
#[test]
fn failed_publication_leaves_a_generation_directory_it_never_created() {
    let output_dir = scratch_path("unowned-generation-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let scene = Scene::MainMenuNewGame;
    let planted = std::cell::RefCell::new(PathBuf::new());
    let take_the_generation_name = || {
        // Whatever name this publish staged under is the name it will rename to.
        let staged = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it renames");
        let generation = staged
            .trim_start_matches('.')
            .trim_end_matches(".staged")
            .to_owned();
        let generation_dir = output_dir.join(generation);
        std::fs::create_dir(&generation_dir).unwrap();
        std::fs::write(generation_dir.join("bystander"), b"not ours").unwrap();
        *planted.borrow_mut() = generation_dir;
        Ok(())
    };

    let error = super::publish_generation(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        take_the_generation_name,
    )
    .unwrap_err();
    assert!(matches!(error, RecordSnapshotError::Write(_, _)), "{error}");

    let planted = planted.borrow().clone();
    assert_eq!(
        std::fs::read(planted.join("bystander")).ok().as_deref(),
        Some(b"not ours".as_slice()),
        "failure cleanup deleted {}, which this publish never created",
        planted.display()
    );

    drop(out_guard);
}

/// Failure cleanup must remove only the staging directory this publish still
/// owns. `create_dir` proves the name was ours when it was taken, not that it
/// is ours when cleanup runs: the name is derived from the scene, the process
/// id, and a counter, so another writer with access to `output_dir` can take
/// it over during the staged write. The recursive removal that follows a
/// failure must then leave that writer's directory alone, exactly as a failed
/// pointer-file publish leaves a replaced staging file alone.
///
/// Not run on Windows: there, `claim_staged_dir`'s exclusive hold denies
/// exactly the rename this test's adversary depends on for as long as this
/// publish still owns the name, so the replacement it stages cannot happen
/// there -- the adversary's own `rename` call fails outright instead.
#[cfg(not(windows))]
#[test]
fn failed_publication_leaves_a_staging_directory_another_writer_replaced() {
    let output_dir = scratch_path("replaced-staging-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let staged_name = |dir: &PathBuf| {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it writes")
    };

    let replaced = std::cell::RefCell::new(PathBuf::new());
    let take_the_staging_name = || {
        let staged_dir = output_dir.join(staged_name(&output_dir));
        // Another writer takes the staging name over and puts its own tree there.
        std::fs::rename(&staged_dir, output_dir.join("carried-off")).unwrap();
        std::fs::create_dir(&staged_dir).unwrap();
        std::fs::write(staged_dir.join("bystander"), b"not ours").unwrap();
        *replaced.borrow_mut() = staged_dir;
        Err(RecordSnapshotError::Write(
            output_dir.join("injected-after-rgb"),
            "injected failure".to_owned(),
        ))
    };

    let error = super::publish_generation(
        Scene::MainMenuNewGame,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        take_the_staging_name,
    )
    .unwrap_err();
    assert!(matches!(error, RecordSnapshotError::Write(_, _)), "{error}");

    let replaced = replaced.borrow().clone();
    assert_eq!(
        std::fs::read(replaced.join("bystander")).ok().as_deref(),
        Some(b"not ours".as_slice()),
        "failure cleanup deleted {}, which this publish no longer owned",
        replaced.display()
    );

    drop(out_guard);
}

/// Failure preserves a replacement even after the original directory has been deleted.
#[cfg(unix)]
#[test]
fn failed_publication_leaves_a_staging_directory_replaced_after_the_original_was_freed() {
    let output_dir = scratch_path("reclaimed-staging-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let replaced = std::cell::RefCell::new(PathBuf::new());
    let take_the_staging_name = || {
        let staged_name = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it writes");
        let staged_dir = output_dir.join(staged_name);
        // Another writer takes the staging name over: it carries this
        // publish's directory off and drops it, then puts its own tree at
        // the name it freed.
        let carried_off = output_dir.join("carried-off");
        std::fs::rename(&staged_dir, &carried_off).unwrap();
        std::fs::remove_dir_all(&carried_off).unwrap();
        std::fs::create_dir(&staged_dir).unwrap();
        std::fs::write(staged_dir.join("bystander"), b"not ours").unwrap();
        *replaced.borrow_mut() = staged_dir;
        Err(RecordSnapshotError::Write(
            output_dir.join("injected-after-rgb"),
            "injected failure".to_owned(),
        ))
    };

    let error = super::publish_generation(
        Scene::MainMenuNewGame,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        take_the_staging_name,
    )
    .unwrap_err();
    assert!(matches!(error, RecordSnapshotError::Write(_, _)), "{error}");

    let replaced = replaced.borrow().clone();
    assert_eq!(
        std::fs::read(replaced.join("bystander")).ok().as_deref(),
        Some(b"not ours".as_slice()),
        "failure cleanup deleted {}, which this publish never created",
        replaced.display()
    );

    drop(out_guard);
}

/// A moved parent must not hide the failed capture's last known staging path.
#[cfg(unix)]
#[test]
fn failed_publication_reports_a_staging_directory_whose_ownership_it_cannot_read() {
    let output_dir = scratch_path("unreadable-claim-out");
    let out_guard = ScratchGuard(output_dir.clone());
    let carried_off = scratch_path("unreadable-claim-carried-off");
    let carried_guard = ScratchGuard(carried_off.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let staged = std::cell::RefCell::new(PathBuf::new());
    let break_the_staging_path = || {
        let name = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it writes");
        *staged.borrow_mut() = output_dir.join(&name);
        // The staging directory survives under `carried_off`, but a
        // non-directory now stands where its path's parent did, so reading
        // its ownership fails with something other than plain absence.
        std::fs::rename(&output_dir, &carried_off).unwrap();
        std::fs::write(&output_dir, b"not a directory").unwrap();
        Err(RecordSnapshotError::Write(
            output_dir.join("injected-after-rgb"),
            "injected failure".to_owned(),
        ))
    };

    let error = super::publish_generation(
        Scene::MainMenuNewGame,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        break_the_staging_path,
    )
    .unwrap_err();

    let staged = staged.borrow().clone();
    let staged_name = staged.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        carried_off.join(&staged_name).is_dir(),
        "the staging directory must still be there for the report to be about anything"
    );
    assert!(
        error.to_string().contains(&staged_name),
        "the error must name the staging directory left behind by a cleanup that could not confirm ownership: {error}"
    );

    drop(carried_guard);
    drop(out_guard);
}

/// Failed directory publication retains the staged payloads and names their location.
#[test]
fn a_failed_promotion_preserves_and_reports_its_staging_directory() {
    let output_dir = scratch_path("failed-promotion-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let staged = std::cell::RefCell::new(PathBuf::new());
    let take_the_generation_name = || {
        let staged_name = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it renames");
        let generation_dir = output_dir.join(
            staged_name
                .trim_start_matches('.')
                .trim_end_matches(".staged"),
        );
        std::fs::create_dir(&generation_dir).unwrap();
        std::fs::write(generation_dir.join("bystander"), b"not ours").unwrap();
        *staged.borrow_mut() = output_dir.join(staged_name);
        Ok(())
    };

    let error = super::publish_generation(
        Scene::MainMenuNewGame,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        take_the_generation_name,
    )
    .unwrap_err();

    let staged = staged.borrow().clone();
    assert!(
        staged.is_dir(),
        "failed promotion must retain staging: {error}"
    );
    assert!(
        error.to_string().contains(&staged.display().to_string()),
        "the error must name the retained staging directory: {error}"
    );
    assert_eq!(
        std::fs::read(staged.join("main-menu-new-game.rgb")).unwrap(),
        b"rgb-bytes"
    );

    drop(out_guard);
}

/// The claim is what carries `create_dir`'s exclusivity forward, so a claim
/// that fails leaves nothing behind to remove by: while it was being taken,
/// another writer can have carried this capture's directory off and put its
/// own tree at the name it freed. Whatever now answers to that name is left
/// alone and named in the reported error instead.
#[test]
fn a_staging_directory_whose_claim_failed_is_reported_rather_than_removed() {
    let output_dir = scratch_path("unclaimable-staging-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let output_claim = super::claim_output_dir(&output_dir).unwrap();

    let staged_dir = output_dir.join(".main-menu-new-game.generation-0-0.staged");
    let take_the_staging_name = |path: &std::path::Path| {
        std::fs::rename(path, output_dir.join("carried-off")).unwrap();
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("bystander"), b"not ours").unwrap();
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    };

    let Err(error) =
        super::create_and_claim_staged_dir(&output_claim, &staged_dir, take_the_staging_name)
    else {
        panic!("a claim that fails must fail the capture");
    };

    assert_eq!(
        std::fs::read(staged_dir.join("bystander")).ok().as_deref(),
        Some(b"not ours".as_slice()),
        "a failed claim removed {}, which it never proved was this capture's",
        staged_dir.display()
    );
    assert!(
        error
            .to_string()
            .contains(&staged_dir.display().to_string()),
        "the error must name the staging directory left behind: {error}"
    );

    drop(out_guard);
}

/// A writer that carries this capture's staging directory off and plants a
/// symlink at the name it freed must not have that link claimed. A claim that
/// followed it would put every payload staged under the staging name into the
/// directory the link points at, anywhere on the filesystem and outside
/// `output_dir` entirely, and would record that directory's identity as this
/// capture's own.
#[cfg(unix)]
#[test]
fn a_staging_name_replaced_by_a_symlink_is_not_claimed() {
    let output_dir = scratch_path("symlinked-staging-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let output_claim = super::claim_output_dir(&output_dir).unwrap();
    let elsewhere = output_dir.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let staged_dir = output_dir.join(".main-menu-new-game.generation-0-0.staged");
    let plant_a_symlink = |path: &std::path::Path| {
        std::fs::remove_dir(path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, path).unwrap();
        output_claim.claim_staged_dir(path)
    };

    let Err(error) =
        super::create_and_claim_staged_dir(&output_claim, &staged_dir, plant_a_symlink)
    else {
        panic!("a symlink at the staging name must fail the claim, and the capture with it");
    };

    assert!(
        error
            .to_string()
            .contains(&staged_dir.display().to_string()),
        "the error must name the staging path: {error}"
    );
    assert!(
        std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "the claim reached through the symlink into {}",
        elsewhere.display()
    );
    assert!(
        std::fs::symlink_metadata(&staged_dir)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a failed claim removed the entry it never proved was this capture's"
    );

    drop(out_guard);
}

/// The Windows counterpart of the symlink test above. A junction needs no
/// privilege to create, so the test never has to skip; claiming one would
/// stage this capture's payloads in the directory it points at, and would
/// leave the hold denying sharing on that directory rather than on the entry
/// cleanup later removes.
#[cfg(windows)]
#[test]
fn a_staging_name_replaced_by_a_junction_is_not_claimed() {
    fn junction(link: &std::path::Path, target: &std::path::Path) {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("cmd runs mklink");
        assert!(
            status.success(),
            "mklink /J {} {}: {status}",
            link.display(),
            target.display()
        );
    }

    let output_dir = scratch_path("junctioned-staging-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let output_claim = super::claim_output_dir(&output_dir).unwrap();
    let elsewhere = output_dir.join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();

    let staged_dir = output_dir.join(".main-menu-new-game.generation-0-0.staged");
    let plant_a_junction = |path: &std::path::Path| {
        std::fs::remove_dir(path).unwrap();
        junction(path, &elsewhere);
        output_claim.claim_staged_dir(path)
    };

    let Err(error) =
        super::create_and_claim_staged_dir(&output_claim, &staged_dir, plant_a_junction)
    else {
        panic!("a junction at the staging name must fail the claim, and the capture with it");
    };

    assert!(
        error
            .to_string()
            .contains(&staged_dir.display().to_string()),
        "the error must name the staging path: {error}"
    );
    assert!(
        std::fs::read_dir(&elsewhere).unwrap().next().is_none(),
        "the claim reached through the junction into {}",
        elsewhere.display()
    );
    assert!(
        std::fs::symlink_metadata(&staged_dir)
            .unwrap()
            .file_type()
            .is_symlink(),
        "a failed claim removed the entry it never proved was this capture's"
    );

    drop(out_guard);
}

/// A staging directory that allows only creation and search, as a `0444`
/// umask leaves it, is claimable through a search-only hold, and the claim
/// that comes back writes its payload through that hold. A claim that cannot
/// write must not report success.
#[cfg(unix)]
#[test]
fn a_staging_directory_that_allows_only_creation_and_search_can_still_be_written() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = scratch_path("search-only-claim");
    let guard = ScratchGuard(dir.clone());
    std::fs::create_dir_all(&dir).unwrap();
    let staged = dir.join(".search-only.staged");
    std::fs::create_dir(&staged).unwrap();
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o333)).unwrap();

    if std::fs::read_dir(&staged).is_ok() {
        eprintln!("note: running privileged, so a 0o333 directory is still readable");
    }
    let claim = super::claim_staged_dir(&staged);
    let written = claim
        .as_ref()
        .ok()
        .map(|claim| claim.write_payload(&staged, "payload", b"held"));
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755)).unwrap();

    claim.expect("a search-only staging directory is still claimable");
    written
        .expect("a claim that never reported success")
        .expect("a claim that succeeded could not write its payload");
    assert_eq!(
        std::fs::read(staged.join("payload")).unwrap(),
        b"held",
        "the payload written through the hold did not land in the claimed directory"
    );
    drop(guard);
}

#[cfg(unix)]
#[test]
fn failed_capture_preserves_every_kind_of_staging_replacement() {
    for kind in ["empty directory", "populated directory", "file", "symlink"] {
        let output = scratch_path(kind);
        let _guard = ScratchGuard(output.clone());
        std::fs::create_dir_all(&output).unwrap();
        let carried = output.join("carried");
        let target = output.join("link-target");
        std::fs::write(&target, b"target").unwrap();
        let mut replaced = PathBuf::new();
        let result =
            super::publish_generation(Scene::MainMenuNewGame, &output, b"rgb", b"meta", || {
                replaced = std::fs::read_dir(&output)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .find(|path| {
                        path.extension()
                            .is_some_and(|extension| extension == "staged")
                    })
                    .unwrap();
                std::fs::rename(&replaced, &carried).unwrap();
                match kind {
                    "empty directory" => std::fs::create_dir(&replaced).unwrap(),
                    "populated directory" => {
                        std::fs::create_dir(&replaced).unwrap();
                        std::fs::write(replaced.join("foreign"), b"foreign").unwrap();
                    }
                    "file" => std::fs::write(&replaced, b"foreign").unwrap(),
                    _ => std::os::unix::fs::symlink(&target, &replaced).unwrap(),
                }
                Err(RecordSnapshotError::Write(
                    replaced.clone(),
                    "capture failed".to_owned(),
                ))
            });
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("capture failed") && error.contains(&replaced.display().to_string())
        );
        match kind {
            "empty directory" => assert_eq!(std::fs::read_dir(&replaced).unwrap().count(), 0),
            "populated directory" => {
                assert_eq!(std::fs::read(replaced.join("foreign")).unwrap(), b"foreign");
            }
            "file" => assert_eq!(std::fs::read(&replaced).unwrap(), b"foreign"),
            _ => assert_eq!(std::fs::read_link(&replaced).unwrap(), target),
        }
        assert_eq!(
            std::fs::read(carried.join("main-menu-new-game.rgb")).unwrap(),
            b"rgb"
        );
        assert_eq!(std::fs::read(&target).unwrap(), b"target");
        assert!(visible_generation(&output, Scene::MainMenuNewGame).is_none());
        assert_eq!(
            std::fs::read_dir(&output).unwrap().count(),
            3,
            "cleanup must not create restore placeholders"
        );
    }
}

/// The output directory pathname is replaced after the claim and before the
/// staging directory exists: staging and payloads must land in the claimed
/// directory, never in whatever the pathname names by then.
#[cfg(unix)]
#[test]
fn staging_lands_in_the_claimed_output_dir_not_a_replacement() {
    for (symlink, swap_after_creation) in [(false, false), (true, false), (false, true)] {
        let root = scratch_path(&format!(
            "staging-parent-swap-{symlink}-{swap_after_creation}"
        ));
        let _guard = ScratchGuard(root.clone());
        let output = root.join("out");
        let carried = root.join("carried");
        let replacement = root.join("replacement");
        std::fs::create_dir_all(&output).unwrap();
        std::fs::create_dir(&replacement).unwrap();

        let output_claim = super::claim_output_dir(&output).unwrap();
        let swap = || {
            std::fs::rename(&output, &carried).unwrap();
            if symlink {
                std::os::unix::fs::symlink(&replacement, &output).unwrap();
            } else {
                std::fs::rename(&replacement, &output).unwrap();
            }
        };
        if !swap_after_creation {
            swap();
        }

        let name = ".main-menu-new-game.generation-0-0.staged";
        let staged = output.join(name);
        let claim = super::create_and_claim_staged_dir(&output_claim, &staged, |path| {
            if swap_after_creation {
                swap();
            }
            output_claim.claim_staged_dir(path)
        })
        .unwrap()
        .expect("the held directory has no staging-name collision");
        claim
            .write_payload(&staged, "main-menu-new-game.rgb", b"rgb")
            .unwrap();
        claim
            .write_payload(&staged, "main-menu-new-game.meta", b"meta")
            .unwrap();

        assert_eq!(
            std::fs::read_dir(&output).unwrap().count(),
            0,
            "the bystander received staging entries or payloads"
        );
        let held = carried.join(name);
        assert_eq!(
            std::fs::read(held.join("main-menu-new-game.rgb")).unwrap(),
            b"rgb"
        );
        assert_eq!(
            std::fs::read(held.join("main-menu-new-game.meta")).unwrap(),
            b"meta"
        );
        claim.require_entry_in(&output_claim, &staged).unwrap();
        assert!(output_claim.require_path(&output).is_err());
    }
}

/// Replacing the staging name with a symlink must not redirect payload writes.
#[cfg(unix)]
#[test]
fn staged_payload_writes_stay_inside_the_output_directory() {
    let output_dir = scratch_path("post-claim-escape-out");
    let out_guard = ScratchGuard(output_dir.clone());
    let escape_dir = scratch_path("post-claim-escape-target");
    let escape_guard = ScratchGuard(escape_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&escape_dir).unwrap();

    let scene = Scene::MainMenuNewGame;
    let plant_a_link_at_the_staging_name = || {
        let staged = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .find(|name| name.starts_with('.') && name.ends_with(".staged"))
            .expect("the publish stages before it writes");
        let staged_dir = output_dir.join(staged);
        // Another writer carries the claimed directory off and leaves a
        // symlink standing at the name this publish keeps writing through.
        std::fs::rename(&staged_dir, output_dir.join("carried-off")).unwrap();
        std::os::unix::fs::symlink(&escape_dir, &staged_dir).unwrap();
        Ok(())
    };

    let _ = super::publish_generation(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        plant_a_link_at_the_staging_name,
    );

    let escaped = escape_dir.join(format!("{}.meta", scene.name()));
    assert!(
        !escaped.exists(),
        "the staged meta write followed a planted link and landed at {}, outside {}",
        escaped.display(),
        output_dir.display()
    );

    drop(escape_guard);
    drop(out_guard);
}

#[cfg(unix)]
#[test]
fn promotion_preserves_an_empty_destination_created_at_the_rename_boundary() {
    use std::os::unix::fs::MetadataExt as _;
    let output = scratch_path("empty-destination-race");
    let _guard = ScratchGuard(output.clone());
    std::fs::create_dir_all(&output).unwrap();
    let staged = output.join("staged");
    let generation = output.join("generation");
    std::fs::create_dir(&staged).unwrap();
    std::fs::write(staged.join("frame.rgb"), b"our capture").unwrap();
    let mut competitor_identity = None;
    let output_claim = super::claim_output_dir(&output).unwrap();
    let result = super::promote_staged_dir(&output_claim, &staged, &generation, || {
        std::fs::create_dir(&generation).unwrap();
        let meta = generation.metadata().unwrap();
        competitor_identity = Some((meta.dev(), meta.ino()));
    });
    assert!(
        result.is_err(),
        "promotion replaced an occupied destination"
    );
    let meta = generation.metadata().unwrap();
    assert_eq!(Some((meta.dev(), meta.ino())), competitor_identity);
    assert_eq!(std::fs::read_dir(&generation).unwrap().count(), 0);
    assert_eq!(
        std::fs::read(staged.join("frame.rgb")).unwrap(),
        b"our capture"
    );
}

/// A competing writer that claims `generation_dir` in the window between the
/// loop's early `exists()` skip and promotion -- here simulated from the
/// `after_rgb_staged` hook, well past that skip -- must not have its
/// directory silently replaced: POSIX `rename(2)` (and, on modern NTFS,
/// `std::fs::rename`'s Windows path too) succeeds when the destination is an
/// empty directory, so promotion must claim `generation_dir` exclusively
/// before ever attempting that rename.
#[test]
fn publish_refuses_to_replace_a_racing_empty_generation_dir() {
    let output_dir = scratch_path("racing-generation-dir-out");
    let out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();

    let scene = Scene::MainMenuNewGame;
    let plant_competing_generation_dir = || {
        // By the time this hook runs, `staged_rgb` has already been written,
        // so this call's own `staged_dir` is the sole `.*.staged` entry;
        // its name is this call's generation, discovered rather than
        // recomputed, since the shared generation counter makes it
        // otherwise unpredictable under parallel tests.
        let generation = std::fs::read_dir(&output_dir)
            .unwrap()
            .find_map(|entry| {
                let name = entry.unwrap().file_name();
                let name = name.to_str()?;
                name.strip_prefix('.')?
                    .strip_suffix(".staged")
                    .map(str::to_owned)
            })
            .expect("this call's own staging directory must exist by now");
        std::fs::create_dir(output_dir.join(&generation)).unwrap();
        Ok(())
    };

    let error = super::publish_generation(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        plant_competing_generation_dir,
    )
    .unwrap_err();

    assert!(matches!(error, RecordSnapshotError::Write(_, _)), "{error}");
    assert!(
        visible_generation(&output_dir, scene).is_none(),
        "a refused promotion must not publish a pointer"
    );
    let generation_dirs: Vec<PathBuf> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.starts_with('.'))
        })
        .collect();
    assert_eq!(
        generation_dirs.len(),
        1,
        "only the competing writer's generation directory may remain: {generation_dirs:?}"
    );
    assert!(
        std::fs::read_dir(&generation_dirs[0])
            .unwrap()
            .next()
            .is_none(),
        "the competing writer's directory must be left exactly as found, still empty"
    );

    drop(out_guard);
}

/// Failure preserves another writer's replacement at the generation name.
#[cfg(unix)]
#[test]
fn failure_cleanup_leaves_a_generation_dir_a_competitor_took_during_publication() {
    let output_dir = scratch_path("replaced-generation-dir-out");
    let _out_guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let scene = Scene::MainMenuNewGame;
    std::fs::create_dir(output_dir.join(format!("{}.generation", scene.name()))).unwrap();

    let (send, recv) = std::sync::mpsc::channel::<PathBuf>();
    let competitor_output_dir = output_dir.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let racer_done = std::sync::Arc::clone(&done);
    let competitor = std::thread::spawn(move || {
        // A publish that fails before the hook drops the sender instead of
        // sending, and one that fails before the rename never produces the
        // payload waited on below; both end this thread rather than parking
        // it forever.
        let Ok(generation_dir) = recv.recv() else {
            return (false, false);
        };
        let generation_rgb = generation_dir.join(format!("{}.rgb", scene.name()));
        let bystander = competitor_output_dir.join(".competitor-directory");
        std::fs::create_dir(&bystander).unwrap();
        std::fs::write(bystander.join("competitor.marker"), b"competitor").unwrap();
        while !generation_rgb.exists() {
            if racer_done.load(std::sync::atomic::Ordering::Relaxed) {
                return (false, false);
            }
            std::hint::spin_loop();
        }
        // The claim has provably happened by now, since this call's own
        // payload is already inside the claimed directory.
        let mut removed = false;
        for _ in 0..1024 {
            if std::fs::remove_dir_all(&generation_dir).is_ok() {
                removed = true;
                break;
            }
            if !generation_dir.exists() {
                break;
            }
        }
        let replaced = std::fs::rename(&bystander, &generation_dir).is_ok();
        (removed, replaced)
    });

    let mut claimed: Option<PathBuf> = None;
    let announce_the_claimed_generation_dir = || {
        let generation = std::fs::read_dir(&output_dir)
            .unwrap()
            .find_map(|entry| {
                let name = entry.unwrap().file_name();
                let name = name.to_str()?;
                name.strip_prefix('.')?
                    .strip_suffix(".staged")
                    .map(str::to_owned)
            })
            .expect("this call's own staging directory must exist by now");
        let generation_dir = output_dir.join(&generation);
        claimed = Some(generation_dir.clone());
        send.send(generation_dir).unwrap();
        Ok(())
    };

    let result = super::publish_generation(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        announce_the_claimed_generation_dir,
    );
    drop(send);
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let (removed, replaced) = competitor.join().unwrap();
    let generation_dir = claimed.expect("the hook must have run");

    assert!(
        result.is_err(),
        "a directory at the pointer path must fail promotion"
    );
    assert!(
        removed && replaced,
        "the competitor must have replaced the claimed directory while this call still held it (removed={removed}, replaced={replaced})"
    );
    assert_eq!(
        std::fs::read(generation_dir.join("competitor.marker")).ok(),
        Some(b"competitor".to_vec()),
        "failure cleanup must not delete a directory this call did not create"
    );
}

/// A competitor spinning on the ordinary way to take a directory name --
/// `remove_dir`, then `rename` its own directory into the freed name --
/// must never come to own `generation_dir` while this call is publishing,
/// and must never receive this call's payloads.
///
/// The public name goes straight from absent to a populated directory,
/// which `remove_dir` refuses, so the competitor's loop has nothing to
/// take.
///
/// Refusals are asserted zero alongside the takeovers because a publish
/// that simply declined to promote would otherwise read as safety.
#[test]
fn promotion_never_writes_payloads_into_a_competitor_owned_generation_dir() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let scene = Scene::MainMenuNewGame;
    let rgb_name = format!("{}.rgb", scene.name());
    let meta_name = format!("{}.meta", scene.name());
    let marker_name = "competitor.marker";
    let mut takeovers = 0u32;
    let mut refusals = 0u32;
    let mut first_refusal = None;
    let mut leaks = 0u32;
    let mut published_leaks = 0u32;

    for attempt in 0..512 {
        let output_dir = scratch_path(&format!("claim-race-out-{attempt}"));
        let _out_guard = ScratchGuard(output_dir.clone());
        std::fs::create_dir_all(&output_dir).unwrap();
        let competitor_dir = output_dir.join(".competitor");
        std::fs::create_dir(&competitor_dir).unwrap();
        // Marks whatever ends up at the generation name as the competitor's
        // directory rather than this call's.
        std::fs::write(competitor_dir.join(marker_name), b"competitor").unwrap();

        let (send, recv) = std::sync::mpsc::channel::<PathBuf>();
        let done = Arc::new(AtomicBool::new(false));
        let racer_done = Arc::clone(&done);
        let competitor = std::thread::spawn(move || {
            // A publish that fails before the hook drops the sender instead
            // of sending, ending this thread rather than parking it forever.
            let Ok(generation_dir) = recv.recv() else {
                return false;
            };
            loop {
                // Only ever takes a directory that is still empty, i.e. one
                // this call created and has not yet promoted into.
                if std::fs::remove_dir(&generation_dir).is_ok() {
                    return std::fs::rename(&competitor_dir, &generation_dir).is_ok();
                }
                if racer_done.load(Ordering::Relaxed) {
                    return false;
                }
                std::hint::spin_loop();
            }
        });

        let mut claimed: Option<PathBuf> = None;
        let announce_the_generation_dir = || {
            let generation = std::fs::read_dir(&output_dir)
                .unwrap()
                .find_map(|entry| {
                    let name = entry.unwrap().file_name();
                    let name = name.to_str()?;
                    name.strip_prefix('.')?
                        .strip_suffix(".staged")
                        .map(str::to_owned)
                })
                .expect("this call's own staging directory must exist by now");
            let generation_dir = output_dir.join(&generation);
            claimed = Some(generation_dir.clone());
            send.send(generation_dir).unwrap();
            Ok(())
        };

        let result = super::publish_generation(
            scene,
            &output_dir,
            b"rgb",
            b"meta",
            announce_the_generation_dir,
        );
        drop(send);
        done.store(true, Ordering::Relaxed);
        if competitor.join().unwrap() {
            takeovers += 1;
        }
        if let Err(error) = &result {
            refusals += 1;
            first_refusal.get_or_insert_with(|| error.to_string());
        }
        let generation_dir = claimed.expect("the hook must have run");
        let entries: Vec<String> = std::fs::read_dir(&generation_dir)
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        if entries.iter().any(|entry| entry == marker_name)
            && entries
                .iter()
                .any(|entry| *entry == rgb_name || *entry == meta_name)
        {
            leaks += 1;
            if result.is_ok() && visible_generation(&output_dir, scene) == Some(generation_dir) {
                published_leaks += 1;
            }
        }
    }

    assert_eq!(
        (takeovers, refusals, leaks, published_leaks),
        (0, 0, 0, 0),
        "a competing writer took the generation name from this call \
         ({takeovers} takeovers, {refusals} promotions refused, {leaks} competitor-owned \
         directories that received this call's payload, {published_leaks} of those then \
         named by the published pointer); first refusal: {first_refusal:?}"
    );
}

#[cfg(unix)]
#[test]
fn a_generation_replacement_cannot_satisfy_the_staged_directory_claim() {
    let output = scratch_path("generation-identity");
    let _guard = ScratchGuard(output.clone());
    std::fs::create_dir_all(&output).unwrap();
    let staged = output.join("staged");
    let generation = output.join("generation");
    let carried = output.join("carried");
    std::fs::create_dir(&staged).unwrap();
    std::fs::write(staged.join("frame"), b"ours").unwrap();
    let claim = super::claim_staged_dir(&staged).unwrap();
    let output_claim = super::claim_output_dir(&output).unwrap();
    super::promote_staged_dir(&output_claim, &staged, &generation, || {}).unwrap();
    claim.require_path(&generation).unwrap();
    std::fs::rename(&generation, &carried).unwrap();
    std::fs::create_dir(&generation).unwrap();
    std::fs::write(generation.join("frame"), b"foreign").unwrap();
    let error = claim.require_path(&generation).unwrap_err();
    assert!(error
        .to_string()
        .contains(&generation.display().to_string()));
    assert_eq!(std::fs::read(generation.join("frame")).unwrap(), b"foreign");
    assert_eq!(std::fs::read(carried.join("frame")).unwrap(), b"ours");
}

#[cfg(unix)]
#[test]
fn a_generation_identity_read_failure_is_reported_with_its_retained_path() {
    let output = scratch_path("unreadable-generation-identity");
    let _guard = ScratchGuard(output.clone());
    let directory = output.join("real").join("generation");
    std::fs::create_dir_all(&directory).unwrap();
    let parent = output.join("parent");
    std::os::unix::fs::symlink(output.join("real"), &parent).unwrap();
    let generation = parent.join("generation");
    let claim = super::claim_staged_dir(&generation).unwrap();
    std::fs::remove_file(&parent).unwrap();
    std::fs::write(&parent, b"foreign").unwrap();
    let source = claim.require_path(&generation).unwrap_err();
    let source = RecordSnapshotError::Write(generation.clone(), source.to_string());
    let report = super::report_retained_directory(&generation, &source).to_string();
    assert!(report.contains(&source.to_string()));
    assert!(report.contains(&generation.display().to_string()));
    assert!(directory.is_dir());
    assert_eq!(std::fs::read(parent).unwrap(), b"foreign");
}

/// Unix keeps the staging handle open across promotion, so a directory swapped
/// in after the last pathname check is promoted to the generation name but
/// never published: the held handle no longer matches it. Both directories
/// survive for inspection.
#[cfg(unix)]
#[test]
fn a_staging_directory_replaced_after_verification_is_promoted_but_never_published() {
    let scene = Scene::MainMenuNewGame;
    let output_dir = scratch_path("staging-replaced-before-rename");
    let _guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let carried = output_dir.join("carried");
    let foreign_marker = "foreign.marker";

    let mut staged_dir = None;
    let swap_the_staging_directory = || {
        let staged = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "staged"))
            .expect("this call's own staging directory must exist by now");
        std::fs::rename(&staged, &carried).unwrap();
        std::fs::create_dir(&staged).unwrap();
        std::fs::write(staged.join(foreign_marker), b"foreign").unwrap();
        staged_dir = Some(staged);
    };

    let error = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        swap_the_staging_directory,
        || {},
        || {},
    )
    .unwrap_err()
    .to_string();
    let staged_dir = staged_dir.expect("the hook must have run");
    let generation_dir = output_dir.join(
        staged_dir
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_start_matches('.')
            .trim_end_matches(".staged"),
    );

    assert!(
        error.contains("no longer matches the capture's held directory")
            && error.contains(&format!("last known path: {}", generation_dir.display())),
        "{error}"
    );
    assert_eq!(visible_generation(&output_dir, scene), None);
    assert!(
        generation_dir.join(foreign_marker).is_file() && !staged_dir.exists(),
        "the replacement was promoted to the generation name and left intact"
    );
    assert_eq!(
        std::fs::read(carried.join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes",
        "this call's payloads stay in its own directory"
    );
}

/// A directory swapped in at `output_dir` after the promoted generation's
/// identity check must not receive a pointer naming a generation it lacks, and
/// its own visible pointer stays as it was.
#[cfg(unix)]
#[test]
fn an_output_directory_swapped_after_the_generation_check_is_never_published_into() {
    let scene = Scene::MainMenuNewGame;
    let root = scratch_path("output-dir-swapped-after-check");
    let _guard = ScratchGuard(root.clone());
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let carried = root.join("carried");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();
    let pointer_name = format!("{}.generation", scene.name());
    std::fs::write(replacement.join(&pointer_name), b"previous\n").unwrap();

    let swap_the_output_directory = || {
        std::fs::rename(&output_dir, &carried).unwrap();
        std::fs::rename(&replacement, &output_dir).unwrap();
    };
    let error = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        || {},
        swap_the_output_directory,
        || {},
    )
    .unwrap_err()
    .to_string();

    assert!(
        error.contains("no longer matches the capture's held directory"),
        "{error}"
    );
    assert_eq!(
        std::fs::read(output_dir.join(&pointer_name)).unwrap(),
        b"previous\n",
        "the visible pointer in the new directory is untouched"
    );
    let generations: Vec<_> = std::fs::read_dir(&carried)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    assert_eq!(generations.len(), 1);
    assert_eq!(
        std::fs::read(generations[0].join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes"
    );
}

/// Swaps a replacement directory in at `output_dir` between the last check
/// and the pointer's rename, optionally carrying the staged file into it, and
/// returns the publication result with the directory the claim verified.
#[cfg(unix)]
fn publish_pointer_across_an_output_dir_swap(
    name: &str,
    carry_staged_file: bool,
) -> (std::io::Result<()>, PathBuf, PathBuf) {
    let root = scratch_path(name);
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let carried = root.join("carried");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();
    let pointer_path = output_dir.join("scene.generation");
    std::fs::write(replacement.join("scene.generation"), b"previous\n").unwrap();

    let claim = super::directory::claim_output_dir(&output_dir).unwrap();
    claim.require_path(&output_dir).unwrap();
    let staged =
        super::stage_pointer(&claim, &pointer_path, b"scene.generation-only-in-out\n").unwrap();
    claim.require_path(&output_dir).unwrap();
    let swap = || {
        if carry_staged_file {
            let staged_name = std::fs::read_dir(&output_dir)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .find(|n| n != "scene.generation")
                .unwrap();
            std::fs::rename(
                output_dir.join(&staged_name),
                replacement.join(&staged_name),
            )
            .unwrap();
        }
        std::fs::rename(&output_dir, &carried).unwrap();
        std::fs::rename(&replacement, &output_dir).unwrap();
    };
    let published = staged.publish_with(&pointer_path, swap, || {});
    (published, output_dir, carried)
}

/// The pointer stages and renames relative to the held directory, so an
/// output directory swapped in after the last check keeps its own pointer and
/// the new pointer lands beside its generation.
#[cfg(unix)]
#[test]
fn an_output_directory_swapped_between_the_last_check_and_the_pointer_rename_keeps_its_pointer() {
    let (published, output_dir, carried) = publish_pointer_across_an_output_dir_swap(
        "output-dir-swapped-before-pointer-rename",
        false,
    );
    let _guard = ScratchGuard(output_dir.parent().unwrap().to_path_buf());
    published.unwrap();
    assert_eq!(
        std::fs::read(output_dir.join("scene.generation")).unwrap(),
        b"previous\n",
        "the swapped-in directory's visible pointer is untouched"
    );
    assert_eq!(
        std::fs::read(carried.join("scene.generation")).unwrap(),
        b"scene.generation-only-in-out\n",
        "the pointer landed in the directory the claim verified"
    );
}

/// A staged file carried into the swapped-in directory is not published from
/// there: the rename looks for it in the held directory and fails.
#[cfg(unix)]
#[test]
fn a_staged_pointer_carried_into_a_swapped_output_directory_is_never_published() {
    let (published, output_dir, _carried) =
        publish_pointer_across_an_output_dir_swap("output-dir-swapped-with-staged-pointer", true);
    let _guard = ScratchGuard(output_dir.parent().unwrap().to_path_buf());
    published.unwrap_err();
    assert_eq!(
        std::fs::read(output_dir.join("scene.generation")).unwrap(),
        b"previous\n",
        "the swapped-in directory's visible pointer is untouched"
    );
}

/// A held output directory whose handle cannot be duplicated stages nothing,
/// so the failure never leaves a pointer file behind under an unpredictable
/// name it does not report.
#[cfg(unix)]
#[test]
fn a_directory_handle_that_cannot_be_duplicated_leaves_no_staged_pointer() {
    let output_dir = scratch_path("pointer-dir-clone-failure");
    let _guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let dir = std::fs::File::open(&output_dir).unwrap();
    let pointer_path = output_dir.join("scene.generation.tmp.0000000001");
    let error = super::staging::stage_in_with(&dir, &pointer_path, b"scene\n", |_| {
        Err(std::io::Error::other("descriptor table full"))
    })
    .err()
    .expect("a failed duplication must fail staging");
    let left: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(
        left.is_empty()
            || error
                .to_string()
                .contains(&pointer_path.display().to_string()),
        "staging left {left:?} behind without reporting it: {error}"
    );
}

/// An output directory swapped out and back (A, then B, then A again) around
/// the generation's promotion and check must not end with A's pointer naming
/// a generation that was promoted into B.
#[cfg(unix)]
#[test]
fn a_generation_promoted_into_a_swapped_output_directory_is_never_published() {
    let scene = Scene::MainMenuNewGame;
    let root = scratch_path("output-dir-swapped-and-restored");
    let _guard = ScratchGuard(root.clone());
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let held_aside = root.join("held-aside");
    let replacement_aside = root.join("replacement-aside");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();
    let pointer_name = format!("{}.generation", scene.name());

    let carry_the_staging_directory_into_a_swapped_replacement = || {
        let staged = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "staged"))
            .expect("this call's own staging directory must exist by now");
        std::fs::rename(&staged, replacement.join(staged.file_name().unwrap())).unwrap();
        std::fs::rename(&output_dir, &held_aside).unwrap();
        std::fs::rename(&replacement, &output_dir).unwrap();
    };
    let restore_the_held_directory = || {
        std::fs::rename(&output_dir, &replacement_aside).unwrap();
        std::fs::rename(&held_aside, &output_dir).unwrap();
    };
    let result = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        carry_the_staging_directory_into_a_swapped_replacement,
        restore_the_held_directory,
        || {},
    );

    for dir in [&output_dir, &held_aside, &replacement_aside] {
        if let Ok(named) = std::fs::read_to_string(dir.join(&pointer_name)) {
            assert!(
                dir.join(named.trim()).is_dir(),
                "the pointer in {} names {:?}, a generation it does not contain",
                dir.display(),
                named.trim()
            );
        }
    }
    let error = result
        .expect_err("a generation promoted outside the held directory must not publish")
        .to_string();
    assert!(error.contains("last known path"), "{error}");
}

/// The promoted generation is looked up in the held output directory, not
/// through the pathname: a generation carried into a directory now at that
/// pathname satisfies the pathname check but not the held one.
#[cfg(unix)]
#[test]
fn a_generation_check_resolves_through_the_held_output_directory() {
    let root = scratch_path("generation-check-held-output");
    let _guard = ScratchGuard(root.clone());
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let held_aside = root.join("held-aside");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();
    let staged = output_dir.join(".generation.staged");
    let generation = output_dir.join("generation");
    std::fs::create_dir(&staged).unwrap();
    let output_claim = super::claim_output_dir(&output_dir).unwrap();
    let claim = super::claim_staged_dir(&staged).unwrap();
    super::promote_staged_dir(&output_claim, &staged, &generation, || {}).unwrap();
    claim.require_entry_in(&output_claim, &generation).unwrap();

    std::fs::rename(&generation, replacement.join("generation")).unwrap();
    std::fs::rename(&output_dir, &held_aside).unwrap();
    std::fs::rename(&replacement, &output_dir).unwrap();

    claim.require_path(&generation).unwrap();
    let error = claim
        .require_entry_in(&output_claim, &generation)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound, "{error}");
}

/// The converse: an output pathname swapped to a directory that lacks the
/// generation leaves the held lookup satisfied, so only a lookup through the
/// pathname, the one the returned payload paths take, can refuse it last.
#[cfg(unix)]
#[test]
fn a_generation_check_through_the_pathname_sees_a_swapped_output_directory() {
    let root = scratch_path("generation-check-swapped-output");
    let _guard = ScratchGuard(root.clone());
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let held_aside = root.join("held-aside");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(replacement.join("generation")).unwrap();
    let staged = output_dir.join(".generation.staged");
    let generation = output_dir.join("generation");
    std::fs::create_dir(&staged).unwrap();
    let output_claim = super::claim_output_dir(&output_dir).unwrap();
    let claim = super::claim_staged_dir(&staged).unwrap();
    super::promote_staged_dir(&output_claim, &staged, &generation, || {}).unwrap();

    std::fs::rename(&output_dir, &held_aside).unwrap();
    std::fs::rename(&replacement, &output_dir).unwrap();

    claim.require_entry_in(&output_claim, &generation).unwrap();
    let error = claim.require_path(&generation).unwrap_err().to_string();
    assert!(
        error.contains("no longer matches the capture's held directory"),
        "{error}"
    );
}

/// A generation replaced after its identity check but before the pointer is
/// published must not be named by the pointer: the check repeats at the last
/// moment, the previous pointer stays intact, and both directories survive.
#[cfg(unix)]
#[test]
fn a_generation_replaced_before_the_pointer_publication_is_never_published() {
    let scene = Scene::MainMenuNewGame;
    let output_dir = scratch_path("generation-replaced-before-pointer-publish");
    let _guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let pointer = output_dir.join(format!("{}.generation", scene.name()));
    std::fs::write(&pointer, b"previous\n").unwrap();
    let carried = output_dir.join("carried");
    let foreign_marker = "foreign.marker";

    let mut generation_dir = None;
    let replace_the_generation = || {
        let generation = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains(".generation-")
                    && path.extension().is_none_or(|ext| ext != "staged")
                    && path.is_dir()
            })
            .expect("the promoted generation must exist by now");
        std::fs::rename(&generation, &carried).unwrap();
        std::fs::create_dir(&generation).unwrap();
        std::fs::write(generation.join(foreign_marker), b"foreign").unwrap();
        generation_dir = Some(generation);
    };

    let error = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        || {},
        || {},
        replace_the_generation,
    )
    .unwrap_err()
    .to_string();
    let generation_dir = generation_dir.expect("the hook must have run");

    assert!(
        error.contains("no longer matches the capture's held directory")
            && error.contains(&format!("last known path: {}", generation_dir.display())),
        "{error}"
    );
    assert_eq!(
        std::fs::read(&pointer).unwrap(),
        b"previous\n",
        "the replaced generation must not be published"
    );
    assert_staged_pointers_reported(&output_dir, &error);
    assert!(generation_dir.join(foreign_marker).is_file());
    assert_eq!(
        std::fs::read(carried.join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes"
    );
}

/// The same replacement with the generation removed outright: the lookup in
/// the held output directory finds nothing, which is also a refusal.
#[cfg(unix)]
#[test]
fn a_generation_moved_away_before_the_pointer_publication_is_never_published() {
    let scene = Scene::MainMenuNewGame;
    let output_dir = scratch_path("generation-moved-before-pointer-publish");
    let _guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let carried = output_dir.join("carried");

    let move_the_generation = || {
        let generation = std::fs::read_dir(&output_dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .contains(".generation-")
                    && path.extension().is_none_or(|ext| ext != "staged")
                    && path.is_dir()
            })
            .expect("the promoted generation must exist by now");
        std::fs::rename(&generation, &carried).unwrap();
    };

    let result = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        || {},
        || {},
        move_the_generation,
    );

    let error = result.unwrap_err().to_string();
    assert!(error.contains("last known path"), "{error}");
    assert_eq!(visible_generation(&output_dir, scene), None);
    assert_staged_pointers_reported(&output_dir, &error);
    assert_eq!(
        std::fs::read(carried.join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes"
    );
}

/// A generation replaced after the pointer already names it cannot be un-published,
/// but success must not be reported for it; the retained path is reported.
#[cfg(unix)]
#[test]
fn a_generation_replaced_after_the_pointer_publication_is_not_reported_as_published() {
    let scene = Scene::MainMenuNewGame;
    let output_dir = scratch_path("generation-replaced-after-pointer-publish");
    let _guard = ScratchGuard(output_dir.clone());
    std::fs::create_dir_all(&output_dir).unwrap();
    let carried = output_dir.join("carried");

    let mut generation_dir = None;
    let replace_the_generation = || {
        let generation = visible_generation(&output_dir, scene)
            .expect("the pointer names the generation by now");
        std::fs::rename(&generation, &carried).unwrap();
        std::fs::create_dir(&generation).unwrap();
        generation_dir = Some(generation);
    };

    let error = super::publish_generation_hooked(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        || {},
        || {},
        || {},
        replace_the_generation,
    )
    .expect_err("a generation replaced after publication must not be reported as published")
    .to_string();
    let generation_dir = generation_dir.expect("the hook must have run");

    assert!(
        error.contains("no longer matches the capture's held directory")
            && error.contains(&format!("last known path: {}", generation_dir.display())),
        "{error}"
    );
    assert_eq!(
        std::fs::read(carried.join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes"
    );
}

/// An output directory swapped out after the last pre-publish check still
/// receives no pointer, since the pointer renames within the held directory; but
/// the returned payload paths are pathnames under `output_dir`, so success is
/// reported only if that pathname still names the held directory.
#[cfg(unix)]
#[test]
fn an_output_directory_swapped_before_the_pointer_publication_is_not_reported_as_published() {
    let scene = Scene::MainMenuNewGame;
    let root = scratch_path("output-dir-swapped-before-pointer-rename-report");
    let _guard = ScratchGuard(root.clone());
    let output_dir = root.join("out");
    let replacement = root.join("replacement");
    let carried = root.join("carried");
    std::fs::create_dir_all(&output_dir).unwrap();
    std::fs::create_dir_all(&replacement).unwrap();

    let swap_the_output_directory = || {
        std::fs::rename(&output_dir, &carried).unwrap();
        std::fs::rename(&replacement, &output_dir).unwrap();
    };
    let result = super::publish_generation_with(
        scene,
        &output_dir,
        b"rgb-bytes",
        b"meta-bytes",
        || Ok(()),
        || {},
        || {},
        swap_the_output_directory,
    );

    if let Ok((rgb, meta)) = &result {
        assert!(
            rgb.is_file() && meta.is_file(),
            "success reported {} and {}, absent from the directory now at output_dir",
            rgb.display(),
            meta.display()
        );
    }
    let error = result
        .expect_err("a swapped output directory must not be reported as published")
        .to_string();
    assert!(
        error.contains("no longer matches the capture's held directory")
            && error.contains("last known path"),
        "{error}"
    );
    let generation =
        visible_generation(&carried, scene).expect("the held directory got its pointer");
    assert_eq!(
        std::fs::read(generation.join(format!("{}.rgb", scene.name()))).unwrap(),
        b"rgb-bytes",
        "the pointer in the held directory names the generation beside it"
    );
}

/// Set in the child [`a_capture_under_an_owner_stripping_umask_publishes_owner_readable_files`]
/// re-runs itself as, so only that child changes its own umask.
#[cfg(target_os = "linux")]
const SNAPSHOT_UMASK_CHILD: &str = "POKEEMERALD_RECORD_SNAPSHOT_UMASK_CHILD";

/// A capture under an owner-read-stripping umask replaces a readable pointer
/// with one its owner can read, and publishes payload files the same way.
/// Group and other stay as the umask left them. Mode bits are asserted so the
/// check still fails as root, which could read a write-only file.
#[cfg(target_os = "linux")]
#[test]
fn a_capture_under_an_owner_stripping_umask_publishes_owner_readable_files() {
    use std::os::unix::fs::PermissionsExt as _;
    let name = "record_snapshot::tests::\
                a_capture_under_an_owner_stripping_umask_publishes_owner_readable_files";
    if std::env::var_os(SNAPSHOT_UMASK_CHILD).is_none() {
        // The umask is process-wide, so the 0o444 one is set only in a
        // re-run of this test binary that runs this test alone.
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", "umask 0444 && exec \"$0\" \"$@\""])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", name, "--test-threads=1"])
            .env(SNAPSHOT_UMASK_CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "umask 0444 child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let output = scratch_path("owner-stripping-umask");
    std::fs::create_dir_all(&output).unwrap();
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _guard = ScratchGuard(output.clone());
    let scene = Scene::MainMenuNewGame;
    let pointer = output.join(format!("{}.generation", scene.name()));
    std::fs::write(&pointer, b"previous-generation\n").unwrap();
    std::fs::set_permissions(&pointer, std::fs::Permissions::from_mode(0o644)).unwrap();

    let (rgb, meta) =
        super::publish_generation(scene, &output, b"rgb", b"meta", || Ok(())).expect("capture");
    let mode = |path: &std::path::Path| {
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    // `0o666` narrowed by umask `0444` is `0o222`. Owner read and write are
    // restored; group and other stay write-only.
    let expected = 0o622;
    let gen_dir = rgb.parent().unwrap();
    assert_eq!(
        mode(gen_dir),
        0o333,
        "the generation directory stays search-only under this umask"
    );
    std::fs::set_permissions(gen_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(mode(&rgb), expected, "rgb payload mode");
    assert_eq!(mode(&meta), expected, "meta payload mode");
    assert_eq!(mode(&pointer), expected, "replacement pointer mode");
    assert_eq!(std::fs::read(&rgb).unwrap(), b"rgb");
    assert_eq!(std::fs::read(&meta).unwrap(), b"meta");
    let published = std::fs::read_to_string(&pointer).unwrap();
    assert_ne!(published, "previous-generation\n");
    assert!(
        published.ends_with('\n') && !published.contains("previous"),
        "pointer was not replaced: {published:?}"
    );
}
