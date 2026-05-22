import { copyFileSync, existsSync, mkdirSync, statSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const scriptDir = dirname(fileURLToPath(import.meta.url))
const appDir = resolve(scriptDir, '..')
const repoRoot = resolve(appDir, '..', '..')
const srcTauriDir = join(appDir, 'src-tauri')

const targetTriple = process.env.TAURI_ENV_TARGET_TRIPLE
  || process.env.CARGO_BUILD_TARGET
  || process.env.TARGET
  || inferHostTargetTriple()

const extension = targetTriple.includes('windows') ? '.exe' : ''
const cliName = `codex-helper${extension}`
const sidecarName = `codex-helper-${targetTriple}${extension}`
const releaseCli = join(repoRoot, 'target', 'release', cliName)
const debugCli = join(repoRoot, 'target', 'debug', cliName)
const sidecarPath = join(srcTauriDir, 'sidecars', sidecarName)

const cliPath = ensureReleaseCli(releaseCli, debugCli)
mkdirSync(dirname(sidecarPath), { recursive: true })
copyFileSync(cliPath, sidecarPath)

const sourceSize = statSync(cliPath).size
console.log(`[prepare-sidecar] copied ${cliPath} -> ${sidecarPath} (${sourceSize} bytes)`)

function ensureReleaseCli(releaseCliPath, debugCliPath) {
  if (process.env.CODEX_HELPER_DESKTOP_SKIP_CLI_BUILD !== '1') {
    console.log('[prepare-sidecar] building release CLI with cargo build --release --bin codex-helper')
    runCargoBuildReleaseCli()
  } else if (existsSync(releaseCliPath)) {
    console.warn('[prepare-sidecar] skipping release CLI rebuild because CODEX_HELPER_DESKTOP_SKIP_CLI_BUILD=1')
  }

  if (existsSync(releaseCliPath)) {
    return releaseCliPath
  }

  console.log('[prepare-sidecar] release CLI is still missing; running cargo build --release --bin codex-helper')
  runCargoBuildReleaseCli()

  if (existsSync(releaseCliPath)) {
    return releaseCliPath
  }

  if (existsSync(debugCliPath) && process.env.CODEX_HELPER_DESKTOP_ALLOW_DEBUG_SIDECAR === '1') {
    console.warn('[prepare-sidecar] using debug CLI because CODEX_HELPER_DESKTOP_ALLOW_DEBUG_SIDECAR=1')
    return debugCliPath
  }

  throw new Error(`codex-helper CLI was not produced at ${releaseCliPath}`)
}

function runCargoBuildReleaseCli() {
  const result = spawnSync('cargo', ['build', '--release', '--bin', 'codex-helper'], {
    cwd: repoRoot,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  })

  if (result.status !== 0) {
    throw new Error(`cargo build --release --bin codex-helper failed with exit code ${result.status}`)
  }
}

function inferHostTargetTriple() {
  const platform = process.platform
  const arch = process.arch

  if (platform === 'win32') {
    if (arch === 'x64') {
      return 'x86_64-pc-windows-msvc'
    }
    if (arch === 'arm64') {
      return 'aarch64-pc-windows-msvc'
    }
  }

  if (platform === 'darwin') {
    if (arch === 'x64') {
      return 'x86_64-apple-darwin'
    }
    if (arch === 'arm64') {
      return 'aarch64-apple-darwin'
    }
  }

  if (platform === 'linux') {
    if (arch === 'x64') {
      return 'x86_64-unknown-linux-gnu'
    }
    if (arch === 'arm64') {
      return 'aarch64-unknown-linux-gnu'
    }
  }

  throw new Error(
    `cannot infer a Tauri sidecar target triple for platform=${platform} arch=${arch}; set TAURI_ENV_TARGET_TRIPLE or CARGO_BUILD_TARGET`,
  )
}
