import "@opentf/web-docs";
import { router } from "@opentf/web";
import { Navbar } from "@opentf/web-docs";
import config from "../otfw.config.js";
import SiteFooter from "../components/SiteFooter.jsx";

export default function RootLayout({ children }) {
  // /docs supplies its own full chrome via DocsLayout (its own navbar, sidebar,
  // TOC), so we omit the marketing shell there to avoid a double navbar.
  // Everything else gets the shared navbar + footer. The check lives inside the
  // returned JSX (not an early return) so client-side navigation reactively
  // swaps chrome.
  const isBare = $derived(router.pathname.startsWith("/docs"));

  return isBare ? (
    <>{children}</>
  ) : (
    <div class="app">
      <Navbar config={config.docs} />

      <main class="main">{children}</main>

      <SiteFooter />
    </div>
  );
}
