use tempfile::{tempdir, TempDir};

pub struct FilesystemRoots {
    pub note_root: TempDir,
    pub temp_root: TempDir,
}

pub fn setup_filesystem() -> FilesystemRoots {
    let note_root = tempdir().expect("could not make temp dir for notes root");
    let temp_root = tempdir().expect("could not make temp dir for temp root");

    std::fs::create_dir(note_root.path().join("notes"))
        .expect("could not make notes dir for testing");
    std::fs::create_dir(note_root.path().join("daily"))
        .expect("could not make daily dir for testing");

    FilesystemRoots {
        note_root,
        temp_root,
    }
}
