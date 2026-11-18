//! The newline a JSON answer ends with.
//!
//! In a terminal the prompt that follows an answer then starts a line of its
//! own, and two answers are two lines.
//!
//! The newline is one more piece after the pieces the answer has. An answer
//! can be as large as a document, so it is not gathered up to be added to,
//! and no part of it is copied: the pieces go on as they came.

use std::future::poll_fn;
use std::pin::Pin;
use std::task::{Context, Poll, ready};

use axum::body::{Body, Bytes};
use axum::extract::Request;
use axum::http::Method;
use axum::http::header::{CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body::{Body as _, Frame, SizeHint};

use super::{ApiError, MCP_PATH};

/// Ends a JSON answer with a newline.
///
/// The MCP endpoint is left as it is: what it sends is the protocol's.
pub(super) async fn end_with_a_newline(request: Request, next: Next) -> Response {
    let leave = request.method() == Method::HEAD || request.uri().path() == MCP_PATH;
    let response = next.run(request).await;
    let json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if leave || !json || response.headers().contains_key(CONTENT_ENCODING) {
        return response;
    }
    let (mut parts, body) = response.into_parts();
    match Ended::after(body).await {
        Ok(None) => Response::from_parts(parts, Body::empty()),
        Ok(Some(ended)) => {
            // The length the answer was given may be one short now. The
            // server writes the length the body says it has, which it knows
            // when the answer was held whole.
            parts.headers.remove(CONTENT_LENGTH);
            Response::from_parts(parts, Body::new(ended))
        }
        Err(_) => ApiError::internal("the answer could not be written").into_response(),
    }
}

/// A body, and a newline after it unless it ends with one.
struct Ended {
    /// A piece that was taken from `rest` and has yet to be handed on.
    held: Option<Frame<Bytes>>,
    rest: Body,
    /// Whether `rest` has given all it has.
    over: bool,
    /// Whether the last byte so far is something other than a newline.
    due: bool,
}

impl Ended {
    /// `body` with a newline to end it, or nothing if `body` is empty.
    ///
    /// The first piece is looked at here. An answer that was held whole is
    /// one piece, so how long it will be with its newline is known before
    /// any of it is sent, and it can be sent with its length.
    async fn after(mut body: Body) -> Result<Option<Self>, axum::Error> {
        let first = poll_fn(|context| Pin::new(&mut body).poll_frame(context));
        let Some(first) = first.await.transpose()? else {
            return Ok(None);
        };
        let mut ended = Self {
            held: None,
            rest: body,
            over: false,
            due: false,
        };
        ended.saw(&first);
        ended.held = Some(first);
        Ok(Some(ended))
    }

    fn saw(&mut self, frame: &Frame<Bytes>) {
        if let Some(last) = frame.data_ref().and_then(|data| data.last()) {
            self.due = *last != b'\n';
        }
    }

    fn newline(&mut self) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        self.due = false;
        Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"\n")))))
    }
}

impl http_body::Body for Ended {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
        let this = self.get_mut();
        if let Some(held) = this.held.take() {
            return Poll::Ready(Some(Ok(held)));
        }
        if this.over {
            return Poll::Ready(None);
        }
        match ready!(Pin::new(&mut this.rest).poll_frame(context)) {
            Some(Ok(frame)) if frame.is_trailers() && this.due => {
                // What follows the data follows the newline too.
                this.held = Some(frame);
                this.newline()
            }
            Some(Ok(frame)) => {
                this.saw(&frame);
                Poll::Ready(Some(Ok(frame)))
            }
            Some(Err(error)) => Poll::Ready(Some(Err(error))),
            None => {
                this.over = true;
                match this.due {
                    true => this.newline(),
                    false => Poll::Ready(None),
                }
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.held.is_none() && !self.due && (self.over || self.rest.is_end_stream())
    }

    fn size_hint(&self) -> SizeHint {
        let held = self
            .held
            .as_ref()
            .and_then(Frame::data_ref)
            .map_or(0, |data| data.len() as u64);
        if self.over || self.rest.is_end_stream() {
            return SizeHint::with_exact(held + u64::from(self.due));
        }
        let rest = self.rest.size_hint();
        let mut hint = SizeHint::new();
        hint.set_lower(held.saturating_add(rest.lower()));
        if let Some(upper) = rest.upper().and_then(|upper| upper.checked_add(held + 1)) {
            hint.set_upper(upper);
        }
        hint
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use axum::http::HeaderMap;

    use super::*;

    /// A body that comes in pieces, as one that is not held whole does.
    struct Pieces(VecDeque<Frame<Bytes>>);

    impl http_body::Body for Pieces {
        type Data = Bytes;
        type Error = axum::Error;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Bytes>, axum::Error>>> {
            Poll::Ready(self.0.pop_front().map(Ok))
        }
    }

    fn pieces(data: &[&'static str], trailers: bool) -> Body {
        let mut frames: VecDeque<Frame<Bytes>> = data
            .iter()
            .map(|piece| Frame::data(Bytes::from_static(piece.as_bytes())))
            .collect();
        if trailers {
            frames.push_back(Frame::trailers(HeaderMap::new()));
        }
        Body::new(Pieces(frames))
    }

    /// The pieces `body` gives, with `None` for what is not data.
    async fn given(mut body: Ended) -> Vec<Option<Bytes>> {
        let mut given = Vec::new();
        while let Some(frame) = poll_fn(|context| Pin::new(&mut body).poll_frame(context)).await {
            given.push(frame.unwrap().into_data().ok());
        }
        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
        given
    }

    fn text(pieces: &[Option<Bytes>]) -> Vec<Option<&str>> {
        pieces
            .iter()
            .map(|piece| {
                piece
                    .as_deref()
                    .map(|data| std::str::from_utf8(data).unwrap())
            })
            .collect()
    }

    #[tokio::test]
    async fn an_answer_held_whole_goes_on_as_it_is_with_its_length_known() {
        let answer = Bytes::from(vec![b'x'; 1 << 20]);
        let (at, length) = (answer.as_ptr(), answer.len());
        let ended = Ended::after(Body::from(answer)).await.unwrap().unwrap();
        // The server sends a length when the body knows its own.
        assert_eq!(ended.size_hint().exact(), Some(length as u64 + 1));
        assert!(!ended.is_end_stream());
        let given = given(ended).await;
        assert_eq!(given.len(), 2);
        // The first piece is the answer where it was, not a copy of it.
        let first = given[0].as_ref().unwrap();
        assert_eq!((first.as_ptr(), first.len()), (at, length));
        assert_eq!(given[1].as_deref(), Some(&b"\n"[..]));
    }

    #[tokio::test]
    async fn an_answer_that_ends_a_line_or_is_empty_gets_nothing() {
        let ended = Ended::after(Body::from("{}\n")).await.unwrap().unwrap();
        assert_eq!(ended.size_hint().exact(), Some(3));
        assert_eq!(text(&given(ended).await), [Some("{}\n")]);
        assert!(Ended::after(Body::empty()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_answer_in_pieces_is_ended_after_the_last_of_them() {
        let ended = Ended::after(pieces(&["{\"a\":", "1", "}"], false));
        let ended = ended.await.unwrap().unwrap();
        // How long it will be is not known until it has all come.
        assert_eq!(ended.size_hint().exact(), None);
        assert_eq!(
            text(&given(ended).await),
            [Some("{\"a\":"), Some("1"), Some("}"), Some("\n")]
        );

        // A newline in the middle is not the end, and one at the end is.
        let ended = Ended::after(pieces(&["{\n", "}\n", ""], false));
        let ended = ended.await.unwrap().unwrap();
        assert_eq!(
            text(&given(ended).await),
            [Some("{\n"), Some("}\n"), Some("")]
        );

        // What follows the data follows the newline.
        let ended = Ended::after(pieces(&["{", "}"], true));
        let ended = ended.await.unwrap().unwrap();
        assert_eq!(
            text(&given(ended).await),
            [Some("{"), Some("}"), Some("\n"), None]
        );
    }
}
