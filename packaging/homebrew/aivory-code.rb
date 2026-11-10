# Homebrew formula for the tap repo aivorynet/homebrew-tap (Formula/aivory-code.rb).
# Template: packaging/render-packages.sh fills the placeholders.
# A formula, not a cask: Homebrew does not set the quarantine flag on formula downloads,
# so Gatekeeper does not block the unsigned binaries.
class AivoryCode < Formula
  desc "War room for coding agents: Claude Code, Codex CLI and Gemini CLI on one screen"
  homepage "https://github.com/aivorynet/aivory-code"
  version "{{VERSION}}"
  license "MIT"

  on_macos do
    on_arm do
      url "https://github.com/aivorynet/aivory-code/releases/download/v{{VERSION}}/aivory-macos-arm64.tar.gz"
      sha256 "{{SHA256_MACOS_ARM64}}"
    end
    on_intel do
      url "https://github.com/aivorynet/aivory-code/releases/download/v{{VERSION}}/aivory-macos-x64.tar.gz"
      sha256 "{{SHA256_MACOS_X64}}"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/aivorynet/aivory-code/releases/download/v{{VERSION}}/aivory-linux-x64.tar.gz"
      sha256 "{{SHA256_LINUX_X64}}"
    end
  end

  def install
    # The app starts the daemon from its own folder, so both stay side by side.
    libexec.install "aivory", "aivoryd"
    bin.install_symlink libexec/"aivory", libexec/"aivoryd"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/aivoryd --version")
  end
end
