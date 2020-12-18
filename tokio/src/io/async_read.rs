use super::ReadBuf;
use std::io;
use std::ops::DerefMut;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::chain::{chain, Chain};
use super::read_to_end::{read_to_end, ReadToEnd};

/// Reads bytes from a source.
///
/// This trait is analogous to the [`std::io::Read`] trait, but integrates with
/// the asynchronous task system. In particular, the [`poll_read`] method,
/// unlike [`Read::read`], will automatically queue the current task for wakeup
/// and return if data is not yet available, rather than blocking the calling
/// thread.
///
/// Specifically, this means that the `poll_read` function will return one of
/// the following:
///
/// * `Poll::Ready(Ok(()))` means that data was immediately read and placed into
///   the output buffer. The amount of data read can be determined by the
///   increase in the length of the slice returned by `ReadBuf::filled`. If the
///   difference is 0, EOF has been reached.
///
/// * `Poll::Pending` means that no data was read into the buffer
///   provided. The I/O object is not currently readable but may become readable
///   in the future. Most importantly, **the current future's task is scheduled
///   to get unparked when the object is readable**. This means that like
///   `Future::poll` you'll receive a notification when the I/O object is
///   readable again.
///
/// * `Poll::Ready(Err(e))` for other errors are standard I/O errors coming from the
///   underlying object.
///
/// This trait importantly means that the `read` method only works in the
/// context of a future's task. The object may panic if used outside of a task.
///
/// Utilities for working with `AsyncRead` values are provided by
/// [`AsyncReadExt`].
///
/// [`poll_read`]: AsyncRead::poll_read
/// [`std::io::Read`]: std::io::Read
/// [`Read::read`]: std::io::Read::read
/// [`AsyncReadExt`]: crate::io::AsyncReadExt
pub trait AsyncRead {
    /// Attempts to read from the `AsyncRead` into `buf`.
    ///
    /// On success, returns `Poll::Ready(Ok(()))` and fills `buf` with data
    /// read. If no data was read (`buf.filled().is_empty()`) it implies that
    /// EOF has been reached.
    ///
    /// If no data is available for reading, the method returns `Poll::Pending`
    /// and arranges for the current task (via `cx.waker()`) to receive a
    /// notification when the object becomes readable or is closed.
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>>;

    /// Creates a new `AsyncRead` instance that chains this stream with
    /// `next`.
    ///
    /// The returned `AsyncRead` instance will first read all bytes from this object
    /// until EOF is encountered. Afterwards the output is equivalent to the
    /// output of `next`.
    ///
    /// # Examples
    ///
    /// [`File`][crate::fs::File]s implement `AsyncRead`:
    ///
    /// ```no_run
    /// use tokio::fs::File;
    /// use tokio::io::{self, AsyncReadExt};
    ///
    /// #[tokio::main]
    /// async fn main() -> io::Result<()> {
    ///     let f1 = File::open("foo.txt").await?;
    ///     let f2 = File::open("bar.txt").await?;
    ///
    ///     let mut handle = f1.chain(f2);
    ///     let mut buffer = String::new();
    ///
    ///     // read the value into a String. We could use any AsyncRead
    ///     // method here, this is just one example.
    ///     handle.read_to_string(&mut buffer).await?;
    ///     Ok(())
    /// }
    /// ```
    fn chain<R>(self, next: R) -> Chain<Self, R>
    where
        Self: Sized,
        R: AsyncRead,
    {
        chain(self, next)
    }

        /// Reads all bytes until EOF in this source, placing them into `buf`.
        ///
        /// Equivalent to:
        ///
        /// ```ignore
        /// async fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize>;
        /// ```
        ///
        /// All bytes read from this source will be appended to the specified
        /// buffer `buf`. This function will continuously call [`read()`] to
        /// append more data to `buf` until [`read()`] returns `Ok(0)`.
        ///
        /// If successful, the total number of bytes read is returned.
        ///
        /// [`read()`]: AsyncReadExt::read
        ///
        /// # Errors
        ///
        /// If a read error is encountered then the `read_to_end` operation
        /// immediately completes. Any bytes which have already been read will
        /// be appended to `buf`.
        ///
        /// # Examples
        ///
        /// [`File`][crate::fs::File]s implement `Read`:
        ///
        /// ```no_run
        /// use tokio::io::{self, AsyncReadExt};
        /// use tokio::fs::File;
        ///
        /// #[tokio::main]
        /// async fn main() -> io::Result<()> {
        ///     let mut f = File::open("foo.txt").await?;
        ///     let mut buffer = Vec::new();
        ///
        ///     // read the whole file
        ///     f.read_to_end(&mut buffer).await?;
        ///     Ok(())
        /// }
        /// ```
        ///
        /// (See also the [`tokio::fs::read`] convenience function for reading from a
        /// file.)
        ///
        /// [`tokio::fs::read`]: fn@crate::fs::read
        fn read_to_end<'a>(&'a mut self, buf: &'a mut Vec<u8>) -> ReadToEnd<'a, Self>
        where
            Self: Unpin,
        {
            read_to_end(self, buf)
        }
}

macro_rules! deref_async_read {
    () => {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut **self).poll_read(cx, buf)
        }
    };
}

impl<T: ?Sized + AsyncRead + Unpin> AsyncRead for Box<T> {
    deref_async_read!();
}

impl<T: ?Sized + AsyncRead + Unpin> AsyncRead for &mut T {
    deref_async_read!();
}

impl<P> AsyncRead for Pin<P>
where
    P: DerefMut + Unpin,
    P::Target: AsyncRead,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.get_mut().as_mut().poll_read(cx, buf)
    }
}

impl AsyncRead for &[u8] {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let amt = std::cmp::min(self.len(), buf.remaining());
        let (a, b) = self.split_at(amt);
        buf.put_slice(a);
        *self = b;
        Poll::Ready(Ok(()))
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncRead for io::Cursor<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let pos = self.position();
        let slice: &[u8] = (*self).get_ref().as_ref();

        // The position could technically be out of bounds, so don't panic...
        if pos > slice.len() as u64 {
            return Poll::Ready(Ok(()));
        }

        let start = pos as usize;
        let amt = std::cmp::min(slice.len() - start, buf.remaining());
        // Add won't overflow because of pos check above.
        let end = start + amt;
        buf.put_slice(&slice[start..end]);
        self.set_position(end as u64);

        Poll::Ready(Ok(()))
    }
}
