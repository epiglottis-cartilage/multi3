use crate::tls::WarpedStream;
use std::sync::{Arc, Mutex};

pub struct ReadHalf {
    inner: Arc<Mutex<WarpedStream>>,
}
impl ReadHalf {
    pub fn inner(&mut self) -> std::sync::MutexGuard<'_, WarpedStream> {
        self.inner.lock().unwrap()
    }
}

pub struct WriteHalf {
    inner: Arc<Mutex<WarpedStream>>,
}
impl WriteHalf {
    pub fn inner(&mut self) -> std::sync::MutexGuard<'_, WarpedStream> {
        self.inner.lock().unwrap()
    }
}

pub fn split(stream: WarpedStream) -> (ReadHalf, WriteHalf) {
    // let is_write_vectored = stream.is_write_vectored();

    let inner = Arc::new(Mutex::new(stream));

    let rd = ReadHalf {
        inner: inner.clone(),
    };

    let wr = WriteHalf { inner };

    (rd, wr)
}
