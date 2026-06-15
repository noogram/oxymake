# Homebrew formula for OxyMake.
#
# This file is the source for the `noogram/homebrew-tap` repository. On each
# release, fill in VERSION and the four SHA256 values from the release assets'
# `*.sha256` files, then commit it to the tap repo so that
# `brew install noogram/tap/oxymake` resolves.
#
# The release workflow (.github/workflows/release.yml) publishes both the
# tarballs and their .sha256 sidecars, so the checksums below are copy-paste.
class Oxymake < Formula
  desc "Content-addressable workflow engine — git checkout no longer rebuilds everything"
  homepage "https://github.com/noogram/oxymake"
  version "0.1.0" # TODO: bump per release
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "https://github.com/noogram/oxymake/releases/download/v#{version}/ox-aarch64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_aarch64-apple-darwin_SHA256"
    end
    on_intel do
      url "https://github.com/noogram/oxymake/releases/download/v#{version}/ox-x86_64-apple-darwin.tar.gz"
      sha256 "REPLACE_WITH_x86_64-apple-darwin_SHA256"
    end
  end

  on_linux do
    on_intel do
      url "https://github.com/noogram/oxymake/releases/download/v#{version}/ox-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "REPLACE_WITH_x86_64-unknown-linux-gnu_SHA256"
    end
  end

  def install
    bin.install "ox"
    # `oxymake` is an alias for `ox`.
    bin.install_symlink "ox" => "oxymake"
  end

  test do
    assert_match "ox", shell_output("#{bin}/ox --help")
  end
end
