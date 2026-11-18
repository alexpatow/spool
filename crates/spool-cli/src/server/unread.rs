//! What becomes of a body the server did not read.
//!
//! A request can be answered before its body was read to the end: nobody was
//! signed in, or the body passed the size limit. What is left of the body is
//! where the next request on the connection would start.
//!
//! A little is read and dropped, and the connection takes the next request.
//!
//! More than that ends the connection, in three steps. The answer says so
//! with `Connection: close`, because a client that is not told sends its next
//! request into a connection that is gone. The server then ends its own side
//! and goes on reading what the client sends, dropping it, until the client
//! has finished or a bound is reached. Only then does it close.
//!
//! The last step is for the answer's sake. A client that is stopped in the
//! middle of writing may never read the answer, and closing a connection
//! with something unread on it resets it, which on some systems, macOS among
//! them, throws away what the client had received and not yet read.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::http::HeaderValue;
use axum::http::header::CONNECTION;
use axum::middleware::Next;
use axum::response::Response;
use axum::serve::{IncomingStream, Listener};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Handle;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// The most of a body that is read and dropped for the connection to go on,
/// and how long the server waits for it to arrive.
const MAX_LEFTOVER_BYTES: usize = 64 * 1024;
const LEFTOVER_WAIT: Duration = Duration::from_secs(2);

/// How much a connection that is ending may still send to be dropped, for
/// how long, and how long it may send nothing. Dropping costs no memory, and
/// a client that sent a body the server would have taken from another caller
/// should get to the end of it.
const MAX_DROPPED_BYTES: u64 = 1 << 30;
const DROPPING_TIME: Duration = Duration::from_secs(30);
const DROPPING_PAUSE: Duration = Duration::from_secs(5);

/// How long a server that is stopping waits for connections that are still
/// being read out. A client that has finished is done in this time, and one
/// that has not does not keep the server from stopping.
const STOPPING_WAIT: Duration = Duration::from_secs(1);

/// Whether a connection was answered with part of a request unread. Its
/// requests carry it, and say so; the connection reads it when it closes.
#[derive(Clone, Default)]
pub(super) struct Unread(Arc<AtomicBool>);

impl Connected<IncomingStream<'_, Connections>> for Unread {
    fn connect_info(stream: IncomingStream<'_, Connections>) -> Self {
        stream.io().unread.clone()
    }
}

/// A listener whose connections read out what was left unread before they
/// close.
pub(super) struct Connections {
    listener: TcpListener,
    closing: mpsc::Sender<()>,
}

impl Connections {
    /// The connections of `listener`, and what waits for the last of them.
    pub(super) fn of(listener: TcpListener) -> (Self, ReadOut) {
        let (closing, closed) = mpsc::channel(1);
        (Self { listener, closing }, ReadOut(closed))
    }
}

/// Ends when the listener is gone and no connection is being read out.
pub(super) struct ReadOut(mpsc::Receiver<()>);

impl ReadOut {
    /// Waits for the connections that are still being read out, for at most
    /// [`STOPPING_WAIT`].
    pub(super) async fn wait(mut self) {
        // Nothing is ever sent: this ends when the last sender is dropped.
        let _ = tokio::time::timeout(STOPPING_WAIT, self.0.recv()).await;
    }
}

impl Listener for Connections {
    type Io = Connection;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Connection, SocketAddr) {
        let (stream, address) = Listener::accept(&mut self.listener).await;
        let connection = Connection {
            stream: Some(stream),
            unread: Unread::default(),
            closing: self.closing.clone(),
        };
        (connection, address)
    }

    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}

pub(super) struct Connection {
    /// There until the connection is dropped.
    stream: Option<TcpStream>,
    unread: Unread,
    closing: mpsc::Sender<()>,
}

impl Connection {
    fn stream(&mut self) -> Pin<&mut TcpStream> {
        Pin::new(self.stream.as_mut().expect("a connection has its stream"))
    }
}

impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        self.stream().poll_read(context, buffer)
    }
}

impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.stream().poll_write(context, bytes)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        slices: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.stream().poll_write_vectored(context, slices)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream
            .as_ref()
            .is_some_and(TcpStream::is_write_vectored)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stream().poll_flush(context)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.stream().poll_shutdown(context)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if !self.unread.0.load(Ordering::Relaxed) {
            return;
        }
        let (Some(stream), Ok(runtime)) = (self.stream.take(), Handle::try_current()) else {
            return;
        };
        let closing = self.closing.clone();
        runtime.spawn(async move {
            drop_the_rest(stream).await;
            drop(closing);
        });
    }
}

/// Ends the server's side of a connection, then reads and drops what the
/// client still sends: until the client has finished, or has sent
/// [`MAX_DROPPED_BYTES`], or [`DROPPING_TIME`] has passed, or nothing has
/// come for [`DROPPING_PAUSE`].
async fn drop_the_rest(mut stream: TcpStream) {
    // The answer was written before the connection was dropped. This tells
    // the client that nothing follows it. It fails if the client has gone,
    // which the read below finds out too.
    let _ = stream.shutdown().await;
    let mut dropped = vec![0; 64 * 1024];
    let end = Instant::now() + DROPPING_TIME;
    let mut left = MAX_DROPPED_BYTES;
    while left > 0 {
        let wait = end.min(Instant::now() + DROPPING_PAUSE);
        match tokio::time::timeout_at(wait, stream.read(&mut dropped)).await {
            Ok(Ok(read)) if read > 0 => left = left.saturating_sub(read as u64),
            _ => return,
        }
    }
}

/// A request's body that whoever answers the request reads through, so that
/// what they left of it can be read afterwards.
struct Shared(Arc<Mutex<Body>>);

impl http_body::Body for Shared {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Bytes>, axum::Error>>> {
        Pin::new(&mut *lock(&self.0)).poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        lock(&self.0).is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        lock(&self.0).size_hint()
    }
}

fn lock(body: &Mutex<Body>) -> MutexGuard<'_, Body> {
    body.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads what is left of a body and drops it. False if more is left than
/// [`MAX_LEFTOVER_BYTES`], or it does not arrive within [`LEFTOVER_WAIT`].
async fn read_out(body: &Mutex<Body>) -> bool {
    use http_body::Body as _;

    let rest = async {
        let mut read = 0;
        loop {
            {
                let body = lock(body);
                if body.is_end_stream() {
                    return true;
                }
                if body.size_hint().lower() > (MAX_LEFTOVER_BYTES - read) as u64 {
                    return false;
                }
            }
            let frame =
                std::future::poll_fn(|context| Pin::new(&mut *lock(body)).poll_frame(context));
            match frame.await {
                None => return true,
                Some(Err(_)) => return false,
                Some(Ok(frame)) => {
                    read += frame.data_ref().map_or(0, Bytes::len);
                    if read > MAX_LEFTOVER_BYTES {
                        return false;
                    }
                }
            }
        }
    };
    tokio::time::timeout(LEFTOVER_WAIT, rest)
        .await
        .unwrap_or(false)
}

/// Leaves the connection fit for the next request, or says that it is not
/// and has the connection read out what is left before it closes.
pub(super) async fn finish_the_body(request: Request, next: Next) -> Response {
    let unread = request
        .extensions()
        .get::<ConnectInfo<Unread>>()
        .map(|ConnectInfo(unread)| unread.clone());
    let (parts, body) = request.into_parts();
    let body = Arc::new(Mutex::new(body));
    let request = Request::from_parts(parts, Body::new(Shared(body.clone())));
    let mut response = next.run(request).await;
    if !read_out(&body).await {
        response
            .headers_mut()
            .insert(CONNECTION, HeaderValue::from_static("close"));
        if let Some(Unread(unread)) = unread {
            unread.store(true, Ordering::Relaxed);
        }
    }
    response
}
