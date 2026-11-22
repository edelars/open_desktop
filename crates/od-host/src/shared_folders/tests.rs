//! Tests of the shared-folder service over real directories (macOS only, like
//! the module). Each test builds `<tmp>/<unique>/{root,outside}`: the service
//! is attached to `root` and `outside/secret.txt` is the object no operation may reach.

use super::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const SECRET: &[u8] = b"outside-the-root";

struct Tree {
    base: PathBuf,
}

impl Tree {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let base = std::env::temp_dir().join(format!(
            "od-shared-folders-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(base.join("root")).unwrap();
        std::fs::create_dir_all(base.join("outside")).unwrap();
        std::fs::write(base.join("outside/secret.txt"), SECRET).unwrap();
        Self { base }
    }

    fn root(&self) -> PathBuf {
        self.base.join("root")
    }

    fn secret(&self) -> PathBuf {
        self.base.join("outside/secret.txt")
    }

    fn root_fd(&self) -> OwnedFd {
        OwnedFd::from(File::open(self.root()).unwrap())
    }

    fn folder(&self, read_only: bool) -> SharedFolder {
        SharedFolder::new(self.root_fd(), read_only).unwrap()
    }

    fn assert_secret_intact(&self) {
        assert_eq!(std::fs::read(self.secret()).unwrap(), SECRET);
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn symlink(target: impl AsRef<Path>, link: impl AsRef<Path>) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn create(folder: &mut SharedFolder, path: &str, data: &[u8]) {
    let handle = folder.open(path, OpenMode::CreateNew).unwrap();
    folder.write(handle, 0, data).unwrap();
    folder.close(handle).unwrap();
}

fn read_all(folder: &mut SharedFolder, path: &str) -> Vec<u8> {
    let handle = folder.open(path, OpenMode::Read).unwrap();
    let data = folder.read(handle, 0, MAX_IO_BYTES).unwrap();
    folder.close(handle).unwrap();
    data
}

/// Every operation the service has, aimed at `path`; each must fail with
/// `expected` and never succeed. The folder is writable so that the failure
/// cannot come from the read-only policy.
fn assert_all_refused(folder: &mut SharedFolder, path: &str, expected: SharedFolderError) {
    assert_eq!(folder.stat(path).unwrap_err(), expected, "stat {path:?}");
    for mode in [
        OpenMode::Read,
        OpenMode::Write,
        OpenMode::Truncate,
        OpenMode::CreateNew,
    ] {
        assert_eq!(
            folder.open(path, mode).unwrap_err(),
            expected,
            "open {mode:?} {path:?}"
        );
    }
    assert_eq!(
        folder.readdir(path, 0, 8).unwrap_err(),
        expected,
        "readdir {path:?}"
    );
    assert_eq!(
        folder.unlink(path).unwrap_err(),
        expected,
        "unlink {path:?}"
    );
    assert_eq!(
        folder.rename(path, "target").unwrap_err(),
        expected,
        "rename from {path:?}"
    );
    assert_eq!(
        folder.rename("seed", path).unwrap_err(),
        expected,
        "rename to {path:?}"
    );
}

#[test]
fn dot_dot_and_dot_components_are_refused_by_every_operation() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "seed", b"seed");
    std::fs::create_dir(tree.root().join("dir")).unwrap();
    for path in [
        "..",
        "../outside/secret.txt",
        "dir/../../outside/secret.txt",
        "dir/..",
        "dir/../seed",
        ".",
        "./seed",
        "dir/./x",
        "dir/../../..",
    ] {
        assert_all_refused(&mut folder, path, SharedFolderError::InvalidPath);
    }
    tree.assert_secret_intact();
    assert!(tree.root().join("seed").exists());
}

#[test]
fn absolute_and_foreign_separator_paths_are_refused() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "seed", b"seed");
    let absolute_secret = tree.secret().to_str().unwrap().to_owned();
    for path in [
        "/",
        "/etc/passwd",
        "//etc/passwd",
        absolute_secret.as_str(),
        "a//b",
        "seed/",
        "..\\outside\\secret.txt",
        "dir\\file",
        "C:\\Windows\\win.ini",
        "C:/Windows",
        "a\u{0}b",
        "a\nb",
    ] {
        assert_all_refused(&mut folder, path, SharedFolderError::InvalidPath);
    }
    // Invalid UTF-8 is a path error too, not a lossy conversion.
    assert_eq!(
        folder.stat(b"seed\xff").unwrap_err(),
        SharedFolderError::InvalidPath
    );
    tree.assert_secret_intact();
}

#[test]
fn symlink_to_root_directory_is_not_followed_anywhere() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    symlink("/", tree.root().join("to_slash"));
    // The link itself is visible and can be inspected.
    let stat = folder.stat("to_slash").unwrap();
    assert_eq!(stat.kind, EntryKind::Symlink);
    for path in ["to_slash", "to_slash/etc/hosts", "to_slash/etc"] {
        assert_eq!(
            folder.open(path, OpenMode::Read).unwrap_err(),
            SharedFolderError::SymlinkRefused,
            "open {path}"
        );
    }
    assert_eq!(
        folder.stat("to_slash/etc/hosts").unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    assert_eq!(
        folder.readdir("to_slash", 0, 8).unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    assert_eq!(
        folder.readdir("to_slash/etc", 0, 8).unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    assert_eq!(
        folder
            .open("to_slash/tmp/x", OpenMode::CreateNew)
            .unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
}

#[test]
fn symlinks_to_outside_objects_are_refused_for_read_write_and_create() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    symlink(tree.secret(), tree.root().join("file_link"));
    symlink(tree.base.join("outside"), tree.root().join("dir_link"));
    symlink("../outside/secret.txt", tree.root().join("relative_link"));
    symlink(
        tree.base.join("outside/new.txt"),
        tree.root().join("dangling"),
    );
    for path in ["file_link", "relative_link"] {
        for mode in [OpenMode::Read, OpenMode::Write, OpenMode::Truncate] {
            assert_eq!(
                folder.open(path, mode).unwrap_err(),
                SharedFolderError::SymlinkRefused,
                "{path} {mode:?}"
            );
        }
    }
    for path in ["dir_link/secret.txt", "dir_link"] {
        assert_eq!(
            folder.open(path, OpenMode::Read).unwrap_err(),
            SharedFolderError::SymlinkRefused
        );
    }
    assert_eq!(
        folder.readdir("dir_link", 0, 8).unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    // A dangling link must not make CreateNew create its target: O_EXCL
    // sees the link itself as an existing name (EEXIST, not ELOOP).
    assert_eq!(
        folder.open("dangling", OpenMode::CreateNew).unwrap_err(),
        SharedFolderError::AlreadyExists
    );
    assert!(!tree.base.join("outside/new.txt").exists());
    tree.assert_secret_intact();
}

#[test]
fn symlink_inside_the_root_pointing_inside_is_refused_too() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "real.txt", b"real");
    symlink("real.txt", tree.root().join("alias"));
    assert_eq!(
        folder.open("alias", OpenMode::Read).unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    assert_eq!(read_all(&mut folder, "real.txt"), b"real");
}

#[test]
fn unlink_and_rename_act_on_the_link_not_on_its_target() {
    let tree = Tree::new();
    let folder = tree.folder(false);
    symlink(tree.secret(), tree.root().join("victim"));
    symlink(tree.secret(), tree.root().join("mover"));
    folder.rename("mover", "moved").unwrap();
    assert_eq!(folder.stat("moved").unwrap().kind, EntryKind::Symlink);
    folder.unlink("victim").unwrap();
    folder.unlink("moved").unwrap();
    assert_eq!(
        folder.stat("victim").unwrap_err(),
        SharedFolderError::NotFound
    );
    tree.assert_secret_intact();
}

#[test]
fn directory_swapped_for_a_symlink_by_the_host_user_is_refused() {
    // A host process replaces a directory with a symlink to an outside
    // directory between guest calls: the next walk refuses the link instead
    // of following it. (A swap in the middle of one walk cannot redirect it
    // either, because each step opens relative to the previous descriptor;
    // that race is not reproduced here.)
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    std::fs::create_dir(tree.root().join("sub")).unwrap();
    create(&mut folder, "sub/inner.txt", b"inner");
    std::fs::rename(tree.root().join("sub"), tree.base.join("moved_away")).unwrap();
    symlink(tree.base.join("outside"), tree.root().join("sub"));
    assert_eq!(
        folder.open("sub/secret.txt", OpenMode::Read).unwrap_err(),
        SharedFolderError::SymlinkRefused
    );
    tree.assert_secret_intact();
}

#[test]
fn unicode_variants_are_ordinary_names_and_never_dot_or_slash() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    create(&mut folder, nfc, b"nfc");
    // Whether the volume folds the two spellings (APFS does) is the volume's
    // behaviour; the service must agree with a direct host lookup and never
    // invent a different answer.
    let host_sees_nfd = tree.root().join(nfd).exists();
    assert_eq!(folder.stat(nfd).is_ok(), host_sees_nfd);
    if host_sees_nfd {
        assert_eq!(read_all(&mut folder, nfd), b"nfc");
        assert_eq!(
            folder.open(nfd, OpenMode::CreateNew).unwrap_err(),
            SharedFolderError::AlreadyExists
        );
    }
    // Compatibility look-alikes of "." and "/" are plain characters.
    for name in [
        "\u{ff0e}\u{ff0e}",
        "a\u{ff0f}b",
        "\u{2024}",
        "e\u{301}\u{ff0e}",
    ] {
        assert_eq!(
            folder.stat(name).unwrap_err(),
            SharedFolderError::NotFound,
            "{name:?}"
        );
        create(&mut folder, name, b"literal");
        assert_eq!(read_all(&mut folder, name), b"literal");
    }
    // Decomposed forms cannot be used to climb either.
    for path in [
        "e\u{301}/..",
        "cafe\u{301}/../..",
        "e\u{301}/../../outside/secret.txt",
    ] {
        assert!(folder.stat(path).is_err(), "{path:?}");
    }
    tree.assert_secret_intact();
}

#[test]
fn long_names_and_paths_are_refused() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let ok = "n".repeat(MAX_NAME_BYTES);
    let too_long = "n".repeat(MAX_NAME_BYTES + 1);
    create(&mut folder, &ok, b"x");
    assert_eq!(
        folder.stat(&too_long).unwrap_err(),
        SharedFolderError::NameTooLong
    );
    assert_eq!(
        folder.open(&too_long, OpenMode::CreateNew).unwrap_err(),
        SharedFolderError::NameTooLong
    );
    assert_eq!(
        folder.stat(format!("dir/{too_long}")).unwrap_err(),
        SharedFolderError::NameTooLong
    );
    // 256 two-byte characters are 512 bytes: the limit is in bytes.
    assert_eq!(
        folder.stat("\u{e9}".repeat(128)).unwrap_err(),
        SharedFolderError::NameTooLong
    );
    let long_path = vec!["abcdefghij"; 120].join("/");
    assert!(long_path.len() > MAX_PATH_BYTES);
    assert_eq!(
        folder.stat(&long_path).unwrap_err(),
        SharedFolderError::PathTooLong
    );
    let deep = vec!["d"; MAX_PATH_DEPTH + 1].join("/");
    assert_eq!(
        folder.stat(&deep).unwrap_err(),
        SharedFolderError::InvalidPath
    );
    assert_eq!(
        folder.stat("a".repeat(MAX_PATH_BYTES + 1)).unwrap_err(),
        SharedFolderError::PathTooLong
    );
}

#[test]
fn read_only_folder_refuses_every_mutation_and_keeps_files() {
    let tree = Tree::new();
    std::fs::write(tree.root().join("keep.txt"), b"keep").unwrap();
    let mut folder = tree.folder(true);
    assert!(folder.info().read_only);
    for mode in [OpenMode::Write, OpenMode::Truncate, OpenMode::CreateNew] {
        assert_eq!(
            folder.open("keep.txt", mode).unwrap_err(),
            SharedFolderError::ReadOnly,
            "{mode:?}"
        );
    }
    assert_eq!(
        folder.open("new.txt", OpenMode::CreateNew).unwrap_err(),
        SharedFolderError::ReadOnly
    );
    assert_eq!(
        folder.unlink("keep.txt").unwrap_err(),
        SharedFolderError::ReadOnly
    );
    assert_eq!(
        folder.rename("keep.txt", "other.txt").unwrap_err(),
        SharedFolderError::ReadOnly
    );
    // Reading is allowed, and a read handle cannot be written through.
    let handle = folder.open("keep.txt", OpenMode::Read).unwrap();
    assert_eq!(folder.read(handle, 0, 16).unwrap(), b"keep");
    assert_eq!(
        folder.write(handle, 0, b"zzzz").unwrap_err(),
        SharedFolderError::ReadOnly
    );
    assert_eq!(
        std::fs::read(tree.root().join("keep.txt")).unwrap(),
        b"keep"
    );
    assert!(!tree.root().join("new.txt").exists());
    assert!(!tree.root().join("other.txt").exists());
}

#[test]
fn read_handle_in_a_writable_folder_cannot_write() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "f", b"data");
    let handle = folder.open("f", OpenMode::Read).unwrap();
    assert_eq!(
        folder.write(handle, 0, b"x").unwrap_err(),
        SharedFolderError::ReadOnly
    );
    assert_eq!(read_all(&mut folder, "f"), b"data");
}

#[test]
fn sequential_writers_last_write_wins() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "shared.bin", &[0u8; 16]);
    let first = folder.open("shared.bin", OpenMode::Write).unwrap();
    let second = folder.open("shared.bin", OpenMode::Write).unwrap();
    folder.write(first, 0, &[1u8; 16]).unwrap();
    folder.write(second, 4, &[2u8; 8]).unwrap();
    folder.write(first, 6, &[3u8; 2]).unwrap();
    let data = folder.read(second, 0, 16).unwrap();
    let mut expected = vec![1u8; 16];
    expected[4..12].fill(2);
    expected[6..8].fill(3);
    assert_eq!(data, expected);
    // No lock is taken: a third open succeeds while two writers are open.
    assert!(folder.open("shared.bin", OpenMode::Write).is_ok());
}

#[test]
fn concurrent_writers_are_uncoordinated_but_never_fail_or_resize() {
    let tree = Tree::new();
    {
        let mut folder = tree.folder(false);
        create(&mut folder, "hot.bin", &vec![0u8; 8192]);
    }
    let threads: Vec<_> = (1u8..=4)
        .map(|pattern| {
            let fd = tree.root_fd();
            std::thread::spawn(move || {
                let mut folder = SharedFolder::new(fd, false).unwrap();
                let handle = folder.open("hot.bin", OpenMode::Write).unwrap();
                for _ in 0..200 {
                    folder.write(handle, 0, &[pattern; 8192][..4096]).unwrap();
                    folder.write(handle, 4096, &[pattern; 4096]).unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let data = std::fs::read(tree.root().join("hot.bin")).unwrap();
    assert_eq!(data.len(), 8192);
    // Each byte comes from exactly one of the writers; whole-range atomicity
    // across overlapping writes is not promised (contract section 8).
    assert!(data.iter().all(|byte| (1..=4).contains(byte)));
}

#[test]
fn concurrent_create_new_has_exactly_one_winner() {
    let tree = Tree::new();
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let fd = tree.root_fd();
            std::thread::spawn(move || {
                let mut folder = SharedFolder::new(fd, false).unwrap();
                folder.open("race.txt", OpenMode::CreateNew).map(|_| ())
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| *e == SharedFolderError::AlreadyExists)
    );
}

#[test]
fn file_lifecycle_stat_read_write_truncate_rename_unlink() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let handle = folder.open("a.txt", OpenMode::CreateNew).unwrap();
    folder.write(handle, 0, b"hello world").unwrap();
    assert_eq!(folder.read(handle, 6, 100).unwrap(), b"world");
    assert_eq!(folder.read(handle, 100, 10).unwrap(), b"");
    folder.write(handle, 20, b"!").unwrap();
    assert_eq!(folder.stat("a.txt").unwrap().size, 21);
    assert_eq!(folder.stat("a.txt").unwrap().kind, EntryKind::File);
    folder.close(handle).unwrap();
    assert_eq!(
        folder.close(handle).unwrap_err(),
        SharedFolderError::BadHandle
    );
    assert_eq!(
        folder.read(handle, 0, 1).unwrap_err(),
        SharedFolderError::BadHandle
    );

    let handle = folder.open("a.txt", OpenMode::Truncate).unwrap();
    assert_eq!(folder.stat("a.txt").unwrap().size, 0);
    folder.write(handle, 0, b"v2").unwrap();
    folder.close(handle).unwrap();

    std::fs::create_dir(tree.root().join("sub")).unwrap();
    folder.rename("a.txt", "sub/b.txt").unwrap();
    assert_eq!(
        folder.stat("a.txt").unwrap_err(),
        SharedFolderError::NotFound
    );
    assert_eq!(read_all(&mut folder, "sub/b.txt"), b"v2");
    // Rename never replaces.
    create(&mut folder, "c.txt", b"c");
    assert_eq!(
        folder.rename("c.txt", "sub/b.txt").unwrap_err(),
        SharedFolderError::AlreadyExists
    );
    assert_eq!(read_all(&mut folder, "sub/b.txt"), b"v2");
    assert_eq!(
        folder.rename("missing", "x").unwrap_err(),
        SharedFolderError::NotFound
    );

    folder.unlink("sub/b.txt").unwrap();
    assert_eq!(
        folder.unlink("sub/b.txt").unwrap_err(),
        SharedFolderError::NotFound
    );
    // A directory is not removed by unlink, and the root has no name to remove.
    assert_eq!(
        folder.unlink("sub").unwrap_err(),
        SharedFolderError::IsADirectory
    );
    assert!(tree.root().join("sub").is_dir());
    assert_eq!(
        folder.unlink("").unwrap_err(),
        SharedFolderError::InvalidPath
    );
    assert_eq!(
        folder.rename("", "x").unwrap_err(),
        SharedFolderError::InvalidPath
    );
    assert_eq!(folder.stat("").unwrap().kind, EntryKind::Directory);
    assert_eq!(
        folder.open("", OpenMode::Read).unwrap_err(),
        SharedFolderError::IsADirectory
    );
    assert_eq!(
        folder.open("sub", OpenMode::Read).unwrap_err(),
        SharedFolderError::IsADirectory
    );
    assert_eq!(
        folder.open("sub", OpenMode::Write).unwrap_err(),
        SharedFolderError::IsADirectory
    );
    assert_eq!(
        folder.open("c.txt/x", OpenMode::Read).unwrap_err(),
        SharedFolderError::NotADirectory
    );
}

#[test]
fn rename_between_hard_links_of_one_file_is_refused() {
    // POSIX rename() of two links to one inode returns 0 and does nothing;
    // the service must refuse and leave both names in place.
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "h1", b"linked");
    std::fs::hard_link(tree.root().join("h1"), tree.root().join("h2")).unwrap();
    assert_eq!(
        folder.rename("h1", "h2").unwrap_err(),
        SharedFolderError::AlreadyExists
    );
    assert_eq!(read_all(&mut folder, "h1"), b"linked");
    assert_eq!(read_all(&mut folder, "h2"), b"linked");
}

#[test]
fn io_limits_are_enforced() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let handle = folder.open("f", OpenMode::CreateNew).unwrap();
    assert_eq!(
        folder.read(handle, 0, MAX_IO_BYTES + 1).unwrap_err(),
        SharedFolderError::TooLarge
    );
    assert_eq!(
        folder
            .write(handle, 0, &vec![0u8; MAX_IO_BYTES + 1])
            .unwrap_err(),
        SharedFolderError::TooLarge
    );
    assert_eq!(
        folder.write(handle, i64::MAX as u64, b"xx").unwrap_err(),
        SharedFolderError::TooLarge
    );
    folder.write(handle, 0, &vec![7u8; MAX_IO_BYTES]).unwrap();
    assert_eq!(
        folder.read(handle, 0, MAX_IO_BYTES).unwrap().len(),
        MAX_IO_BYTES
    );
}

#[test]
fn open_file_limit_and_close_all() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    create(&mut folder, "f", b"x");
    let handles: Vec<_> = (0..MAX_OPEN_FILES)
        .map(|_| folder.open("f", OpenMode::Read).unwrap())
        .collect();
    assert_eq!(
        folder.open("f", OpenMode::Read).unwrap_err(),
        SharedFolderError::TooManyOpenFiles
    );
    folder.close(handles[0]).unwrap();
    let again = folder.open("f", OpenMode::Read).unwrap();
    assert!(!handles[1..].contains(&again));
    folder.close_all();
    assert_eq!(folder.open_file_count(), 0);
    assert_eq!(
        folder.read(handles[1], 0, 1).unwrap_err(),
        SharedFolderError::BadHandle
    );
}

#[test]
fn readdir_is_sorted_paged_and_skips_unrepresentable_names() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    for name in ["c", "a", "b", "e", "d"] {
        create(&mut folder, name, b"");
    }
    std::fs::create_dir(tree.root().join("dir")).unwrap();
    symlink("a", tree.root().join("link"));
    // A name the guest cannot be given: a colon. (APFS itself refuses names
    // that are not valid UTF-8, so that case cannot be built here; the filter
    // is the same function and covers volumes that allow such names.)
    std::fs::write(tree.root().join("has:colon"), b"").unwrap();

    let mut names = Vec::new();
    let mut kinds = Vec::new();
    let mut cookie = 0u32;
    let mut pages = 0;
    loop {
        let page = folder.readdir("", cookie, 3).unwrap();
        assert!(page.entries.len() <= 3);
        assert_eq!(page.skipped, 1);
        for entry in &page.entries {
            names.push(entry.name.clone());
            kinds.push(entry.kind);
        }
        pages += 1;
        match page.next {
            Some(next) => cookie = next,
            None => break,
        }
    }
    assert_eq!(names, ["a", "b", "c", "d", "dir", "e", "link"]);
    assert_eq!(kinds[4], EntryKind::Directory);
    assert_eq!(kinds[6], EntryKind::Symlink);
    assert_eq!(kinds[0], EntryKind::File);
    assert_eq!(pages, 3);
    // Listing twice from the same root works (no shared directory offset).
    assert_eq!(folder.readdir("", 0, 64).unwrap().entries.len(), 7);
    assert_eq!(folder.readdir("", 7, 3).unwrap().entries.len(), 0);
    assert_eq!(
        folder.readdir("", 0, 0).unwrap_err(),
        SharedFolderError::TooLarge
    );
    assert_eq!(
        folder.readdir("", 0, MAX_READDIR_PAGE + 1).unwrap_err(),
        SharedFolderError::TooLarge
    );
    assert_eq!(
        folder.readdir("a", 0, 4).unwrap_err(),
        SharedFolderError::NotADirectory
    );
}

#[test]
fn case_behaviour_matches_the_probed_volume() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let info = folder.info();
    create(&mut folder, "File.TXT", b"x");
    // Case is preserved in listings whatever the volume does with lookups.
    if info.case_preserving {
        let names: Vec<_> = folder
            .readdir("", 0, 8)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["File.TXT"]);
    }
    if info.case_sensitive {
        assert_eq!(
            folder.stat("file.txt").unwrap_err(),
            SharedFolderError::NotFound
        );
        create(&mut folder, "file.txt", b"y");
    } else {
        assert!(folder.stat("file.txt").is_ok());
        assert_eq!(
            folder.open("file.txt", OpenMode::CreateNew).unwrap_err(),
            SharedFolderError::AlreadyExists
        );
        // Case-only rename is allowed and keeps one file.
        folder.rename("File.TXT", "FILE.txt").unwrap();
        let names: Vec<_> = folder
            .readdir("", 0, 8)
            .unwrap()
            .entries
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["FILE.txt"]);
    }
}

#[test]
fn fifo_in_the_folder_does_not_block_and_is_not_openable() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    let status = std::process::Command::new("mkfifo")
        .arg(tree.root().join("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(folder.stat("pipe").unwrap().kind, EntryKind::Other);
    for mode in [OpenMode::Read, OpenMode::Write, OpenMode::Truncate] {
        assert_eq!(
            folder.open("pipe", mode).unwrap_err(),
            SharedFolderError::UnsupportedKind,
            "{mode:?}"
        );
    }
}

#[test]
fn root_must_be_a_directory_and_descriptor_is_close_on_exec() {
    let tree = Tree::new();
    std::fs::write(tree.root().join("plain"), b"x").unwrap();
    let file_fd = OwnedFd::from(File::open(tree.root().join("plain")).unwrap());
    assert_eq!(
        SharedFolder::new(file_fd, false).err(),
        Some(SharedFolderError::NotADirectory)
    );
    let root = tree.root_fd();
    let raw = root.as_raw_fd();
    let _folder = SharedFolder::new(root, false).unwrap();
    // SAFETY: `raw` is still owned by `_folder`; F_GETFD reads no memory.
    let flags = unsafe { fcntl(raw, 1) };
    assert!(flags >= 0 && flags & FD_CLOEXEC != 0);
}

#[test]
fn nested_paths_resolve_below_the_root() {
    let tree = Tree::new();
    let mut folder = tree.folder(false);
    std::fs::create_dir_all(tree.root().join("a/b/c")).unwrap();
    create(&mut folder, "a/b/c/deep.txt", b"deep");
    assert_eq!(read_all(&mut folder, "a/b/c/deep.txt"), b"deep");
    assert_eq!(folder.stat("a/b").unwrap().kind, EntryKind::Directory);
    assert_eq!(
        folder.readdir("a/b/c", 0, 8).unwrap().entries,
        vec![DirEntry {
            name: "deep.txt".to_owned(),
            kind: EntryKind::File
        }]
    );
    assert_eq!(
        folder.stat("a/nope/c").unwrap_err(),
        SharedFolderError::NotFound
    );
}
