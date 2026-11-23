//! The image backup page: a partition written out whole as an image file, an
//! image written back over a partition, and the images kept that are no
//! longer wanted.
//!
//! An image is the partition's own bytes, start to end, with nothing added:
//! writing one back puts the partition in exactly the state it was in when
//! it was taken. The digest of the whole image is taken as it is written, so
//! an image written back is checked against the one that was saved before a
//! single byte of the partition is touched.

use super::*;

/// A partition an image can be taken from or written to, as the runtime
/// found it: its name, and how large the partition is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImagePart {
    pub name: String,
    /// The partition's size in bytes, which is the image's size too.
    pub size: u64,
}

/// An image already saved, as the page lists it: its file, the partition it
/// was taken from, and how large it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageFile {
    /// The file's name, which is also what it is listed by.
    pub name: String,
    /// The partition the image was taken from, where the name still says it.
    pub partition: String,
    /// Where the file sits, for the runtime to write or delete.
    pub path: String,
    /// The digest of the whole image, as it was taken: what writing it back
    /// is checked against before the partition is touched.
    pub sha256: String,
    pub size: u64,
    /// When it was written, in seconds since the epoch.
    pub made: i64,
}

/// What the image backup page is doing, or how it last ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageJob {
    /// Whether this is an image being written out or written back.
    pub kind: ImageFlow,
    /// The partition the work is on, and how many of its bytes are done.
    pub partition: String,
    pub done: u64,
    pub of: u64,
    /// Asked to stop, and not yet ended.
    pub stopping: bool,
    /// How it ended, once it has.
    pub outcome: Option<ImageEnd>,
}

/// Whether an image is being taken from a partition or put back over one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFlow {
    /// Written out to a file.
    Export,
    /// Written back over the partition from a file.
    Import,
}

/// How work on an image ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageEnd {
    /// The image was written, at this path.
    Exported(String),
    /// The image was written back over the partition.
    Imported(String),
    /// The image was deleted.
    Deleted(String),
    /// Stopped as asked, with the half-written image taken away again.
    Stopped,
    /// The image does not hold what it should, so nothing was written.
    Mismatched { wanted: String, found: String },
    Failed(String),
}

impl App {
    /// The runtime read the partitions an image can be taken from, and the
    /// images already saved, or could not: keep them and show them.
    pub fn set_images(&mut self, parts: Result<Vec<ImagePart>, String>, files: Result<Vec<ImageFile>, String>) {
        if self.state.image_parts != parts {
            self.state.image_parts = parts;
            self.refresh();
        }
        if self.state.image_files != files {
            self.state.image_files = files;
            self.refresh();
        }
    }

    /// Work on an image has begun, or has moved on: how far it has come.
    pub fn image_at(&mut self, kind: ImageFlow, partition: &str, done: u64, of: u64) {
        self.state.image_job = Some(ImageJob { kind, partition: partition.to_owned(), done, of, stopping: false, outcome: None });
        self.refresh();
    }

    /// Work on an image has ended, and how.
    pub fn image_ended(&mut self, end: ImageEnd) {
        if let Some(job) = self.state.image_job.as_mut() {
            job.outcome = Some(end);
            // A finished job that went through leaves the list of images
            // saying what it was, so the runtime reads it again.
            self.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::testing::*;

    #[test]
    fn the_size_of_an_image_is_the_size_of_its_partition() {
        let parts = vec![ImagePart { name: "boot_a".into(), size: 100 * 1024 * 1024 }];
        let mut app = App::new(Screen::ImageBackup, 1440, 3200, 3.5);
        app.set_images(Ok(parts.clone()), Ok(Vec::new()));
        assert_eq!(app.state.image_parts, Ok(parts), "the partitions read are the ones shown");
        assert!(app.state.image_files == Ok(Vec::new()), "and no images are saved yet");
    }

    #[test]
    fn a_job_says_how_far_it_has_come_and_how_it_ended() {
        let mut app = App::new(Screen::ImageBackup, 1440, 3200, 3.5);
        app.image_at(ImageFlow::Export, "boot_a", 0, 1000);
        let job = app.state.image_job.as_ref().expect("a job under way");
        assert_eq!((job.done, job.of), (0, 1000), "nothing written yet");
        assert!(job.outcome.is_none(), "and it has not ended");
        assert!(!job.stopping, "and nothing has asked it to stop");
        app.image_at(ImageFlow::Export, "boot_a", 400, 1000);
        assert_eq!(app.state.image_job.as_ref().unwrap().done, 400, "it moves on");
        app.image_ended(ImageEnd::Exported("/data/media/0/skyward/boot_a.img".into()));
        let job = app.state.image_job.as_ref().expect("the job is kept to say how it ended");
        assert_eq!(job.outcome, Some(ImageEnd::Exported("/data/media/0/skyward/boot_a.img".into())));
    }

    #[test]
    fn an_image_that_does_not_hold_what_it_should_writes_nothing() {
        // The two digests are both said, so the page can show what was wanted
        // beside what was found.
        let end = ImageEnd::Mismatched { wanted: "ab".repeat(32), found: "cd".repeat(32) };
        let mut app = App::new(Screen::ImageBackup, 1440, 3200, 3.5);
        app.image_at(ImageFlow::Import, "boot_a", 0, 1000);
        app.image_ended(end.clone());
        assert_eq!(app.state.image_job.unwrap().outcome, Some(end), "nothing was written, and it says why");
    }

    #[test]
    fn a_tap_on_a_partition_asks_with_a_swipe_before_anything_is_written() {
        let (f, i) = (fonts(), info());
        let parts = vec![ImagePart { name: "boot_a".into(), size: 100 * 1024 * 1024 }];
        let mut app = App::new(Screen::ImageBackup, W, H, PPD);
        app.set_images(Ok(parts), Ok(Vec::new()));
        let row = app.frame(&f, &i).hits.iter().find(|h| h.action == Action::ExportImage(0)).expect("the partition's row").rect;
        let (x, y) = (row.x + row.w / 2.0, row.y + row.h / 2.0);
        assert!(app.touch(Touch::Down(x, y), &f, 0).is_empty(), "the finger landing writes nothing");
        assert!(app.touch(Touch::Up, &f, 0).is_empty(), "nor letting go of it");
        assert!(app.state.image_job.is_none(), "no image is being written");
        assert_eq!(app.state.image_asking, Some(Confirm::ExportImage(0)), "the swipe asks first");
    }

    #[test]
    fn the_swipe_on_a_partition_writes_it_out_and_a_tap_alone_does_not() {
        let (f, i) = (fonts(), info());
        let parts = vec![ImagePart { name: "boot_a".into(), size: 100 * 1024 * 1024 }];
        let mut app = App::new(Screen::ImageBackup, W, H, PPD);
        app.set_images(Ok(parts), Ok(Vec::new()));
        let row = app.frame(&f, &i).hits.iter().find(|h| h.action == Action::ExportImage(0)).expect("the partition's row").rect;
        app.touch(Touch::Down(row.x + row.w / 2.0, row.y + row.h / 2.0), &f, 0);
        app.touch(Touch::Up, &f, 0);
        assert_eq!(swipe(&mut app, &f, &i, Confirm::ExportImage(0)), vec![Effect::ExportImage {
            partition: "boot_a".into(),
            size: 100 * 1024 * 1024,
            drive: false,
        }]);
        assert!(app.state.image_asking.is_none(), "and the card goes with it");
    }

    #[test]
    fn an_image_asks_before_it_is_written_back_or_deleted() {
        let (f, i) = (fonts(), info());
        let file = ImageFile {
            name: "boot_a.img".into(),
            partition: "boot_a".into(),
            path: "/data/media/0/skyward/boot_a.img".into(),
            sha256: "ab".repeat(32),
            size: 4096,
            made: 1_792_348_260,
        };
        let listing = || {
            let mut app = App::new(Screen::ImageBackup, W, H, PPD);
            app.set_images(Ok(Vec::new()), Ok(vec![file.clone()]));
            app.state.image_showing_parts = false;
            app.frame(&f, &i);
            app
        };
        let mut app = listing();
        tap(&mut app, &f, &i, Action::ImportImage(0));
        assert!(app.state.image_job.is_none(), "a tap on the image writes nothing");
        assert_eq!(app.state.image_asking, Some(Confirm::ImportImage(0)), "it asks with a swipe");
        assert_eq!(swipe(&mut app, &f, &i, Confirm::ImportImage(0)), vec![Effect::ImportImage {
            partition: "boot_a".into(),
            path: "/data/media/0/skyward/boot_a.img".into(),
            sha256: "ab".repeat(32),
        }]);
        // The image being written back is the page now, so deleting one is
        // asked on the list afresh.
        let mut app = listing();
        tap(&mut app, &f, &i, Action::DeleteImage(0));
        assert!(app.state.image_files == Ok(vec![file.clone()]), "a tap on delete deletes nothing");
        assert_eq!(app.state.image_asking, Some(Confirm::DeleteImage(0)));
        assert_eq!(swipe(&mut app, &f, &i, Confirm::DeleteImage(0)), vec![Effect::DeleteImage("/data/media/0/skyward/boot_a.img".into())]);
    }

    #[test]
    fn a_tap_away_puts_the_image_question_away() {
        let (f, i) = (fonts(), info());
        let parts = vec![ImagePart { name: "boot_a".into(), size: 100 * 1024 * 1024 }];
        let mut app = App::new(Screen::ImageBackup, W, H, PPD);
        app.set_images(Ok(parts), Ok(Vec::new()));
        tap(&mut app, &f, &i, Action::ExportImage(0));
        assert!(app.state.image_asking.is_some());
        // The swipe stands under the list as the page draws it: a tap that
        // lands anywhere else puts the question away and writes nothing.
        app.touch(Touch::Down(W as f32 / 2.0, 10.0), &f, 0);
        assert!(app.state.image_asking.is_none(), "a tap elsewhere puts it away");
        assert!(app.state.image_job.is_none(), "and writes nothing");
    }
}
