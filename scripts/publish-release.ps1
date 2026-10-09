# Publishes the built openmouse.exe to GitHub Releases.
#
# - Triggered by a v* tag (push or manual input): versioned release.
# - Otherwise (push/merge to main): rolling "latest" prerelease.
#
# Expects to run from the repo root after `cargo build --release`.
# Uses the standard GitHub Actions env vars plus:
#   GITHUB_TOKEN       (from secrets)
#   RELEASE_TAG_INPUT  (manual version tag, may be empty)

$ErrorActionPreference = "Continue"

$exe = "target/release/openmouse.exe"
if (-not (Test-Path $exe)) { throw "Build output missing: $exe" }

$sha = $env:GITHUB_SHA.Substring(0, 7)
$manualTag = $env:RELEASE_TAG_INPUT

if ($env:GITHUB_EVENT_NAME -eq "workflow_dispatch" -and $manualTag -ne "") {
    $tag = $manualTag; $isLatest = $false
} elseif ($env:GITHUB_REF_TYPE -eq "tag") {
    $tag = $env:GITHUB_REF_NAME; $isLatest = $false
} else {
    $tag = "latest"; $isLatest = $true
}

if ($isLatest) {
    # Rolling build: replace the previous 'latest' release entirely.
    gh release delete latest --yes 2>$null
    git push origin ":refs/tags/latest" 2>$null
    gh release create latest --prerelease `
        --title "OpenMouse (latest build)" `
        --notes "Automatic build from main ($sha). Download openmouse.exe, run it, and scan the QR code with your phone (same WiFi). Nothing to install." `
        $exe
} else {
    gh release view $tag 2>$null
    if ($LASTEXITCODE -ne 0) {
        gh release create $tag --title "OpenMouse $tag" `
            --notes "Windows standalone build. Download openmouse.exe, run it, and scan the QR code with your phone (same WiFi). Nothing to install."
    }
    gh release upload $tag $exe --clobber
}
