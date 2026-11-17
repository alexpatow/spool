// Gives the tests a document to render the studio into.
//
// The studio is tested against the real server, so the calls it makes go out
// with the `fetch` of the runtime, which is kept here: the one a simulated
// browser brings applies the rules of a page, such as those about origins,
// to a test that is not one.

import { GlobalRegistrator } from "@happy-dom/global-registrator";

const kept = {
  fetch: globalThis.fetch,
  Request: globalThis.Request,
  Response: globalThis.Response,
  Headers: globalThis.Headers,
  AbortController: globalThis.AbortController,
  AbortSignal: globalThis.AbortSignal,
};

GlobalRegistrator.register({ url: "http://studio.test/" });
Object.assign(globalThis, kept);

// Tells React that what the tests do to the page is to be waited for.
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
