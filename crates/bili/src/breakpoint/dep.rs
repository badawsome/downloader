use crate::VideoId;

pub trait Progresser {
    fn start(id: &VideoId, full: u64);
    fn changed(id: &VideoId, now: u64);
    async fn done(id: &VideoId);
}
