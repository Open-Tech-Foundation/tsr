import { Tabs, CodeBlock } from "@opentf/web-docs";

export default function InstallTabs() {
  const installTabs = [
    {
      label: "Linux / macOS / FreeBSD",
      content: (
        <CodeBlock
          lang="sh"
          code="curl -fsSL https://raw.githubusercontent.com/Open-Tech-Foundation/tsr/main/install.sh | bash"
        />
      ),
    },
    {
      label: "Windows (PowerShell)",
      content: (
        <CodeBlock
          lang="powershell"
          code="irm https://raw.githubusercontent.com/Open-Tech-Foundation/tsr/main/install.ps1 | iex"
        />
      ),
    },
    {
      label: "From Source (Cargo)",
      content: (
        <CodeBlock
          lang="sh"
          code="cargo build --release"
        />
      ),
    },
  ];

  return <Tabs tabs={installTabs} />;
}
