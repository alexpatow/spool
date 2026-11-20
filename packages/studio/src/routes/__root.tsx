// What is around every screen. The router shows the screen an address
// names inside it, or says that none has that address.

import { createRootRoute } from "@tanstack/react-router";

import { Nowhere, Shell } from "../app/shell.tsx";

export const Route = createRootRoute({ component: Shell, notFoundComponent: Nowhere });
