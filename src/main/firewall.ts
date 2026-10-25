import { execFile } from 'node:child_process'
import { writeFile, unlink } from 'node:fs/promises'
import { join } from 'node:path'
import { app } from 'electron'
import type { FirewallStatus, NetworkProfile } from '@shared/ipc'

/**
 * Regra de firewall de entrada para quem transmite.
 *
 * Quem so assiste nao precisa de nada: e uma conexao de saida. Quem transmite
 * roda um servidor (WS de sinalizacao + UDP de descoberta + ICE), e o Windows
 * bloqueia isso por padrao.
 *
 * A leitura usa cmdlets do PowerShell porque devolvem objetos: a saida do
 * `netsh` e traduzida para o idioma do sistema e nao da para fazer parsing
 * confiavel em um Windows em portugues. Ja a criacao usa `netsh`, que e o
 * comando que aparece na UI como alternativa manual.
 */

export const RULE_NAME = 'LAN ScreenShare'

const PS_ARGS = ['-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command']

function run(file: string, args: string[], timeout = 15_000): Promise<{ stdout: string }> {
  return new Promise((resolve, reject) => {
    execFile(file, args, { timeout, windowsHide: true }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(stderr?.trim() || err.message))
        return
      }
      resolve({ stdout })
    })
  })
}

/** Caminho do executavel a liberar. No build portatil e o exe extraido. */
function programPath(): string {
  return process.execPath
}

/**
 * `profile=any` cobre tambem o perfil Publico.
 *
 * Adaptadores de VPN (Radmin, Hamachi, ZeroTier) sao quase sempre classificados
 * como Publica pelo Windows, e e justamente por eles que passa o caso de
 * assistir com alguem que nao esta na mesma casa. Restringir a `private` faria
 * a regra existir e a conexao falhar mesmo assim.
 *
 * O que protege a transmissao e o codigo de acesso, nao o perfil de rede.
 */
export function firewallCommand(): string {
  if (process.platform === 'linux') {
    return 'sudo ufw allow 41234/udp && sudo ufw allow 41235/tcp'
  }
  return `netsh advfirewall firewall add rule name="${RULE_NAME}" dir=in action=allow program="${programPath()}" enable=yes profile=any`
}

const PROFILE_MAP: Record<string, NetworkProfile> = {
  Public: 'public',
  Private: 'private',
  DomainAuthenticated: 'domain'
}

interface ProbeResult {
  programs: string[]
  categories: string[]
}

/**
 * Filtrar direcao com `Where-Object`, e nao com `-Direction`.
 *
 * `Get-NetFirewallRule -DisplayName X -Direction Inbound` e um conjunto de
 * parametros ambiguo e falha; com ErrorActionPreference silencioso o erro some
 * e a checagem passa a responder "nao existe" para sempre.
 *
 * Devolve todos os programas das regras habilitadas em vez de so a primeira: o
 * Windows cria uma regra por protocolo (TCP e UDP) quando o usuario responde ao
 * aviso nativo, entao a primeira pode nao ser a nossa.
 */
const PROBE_SCRIPT = `
$ErrorActionPreference = 'SilentlyContinue'
$programs = @(
  Get-NetFirewallRule -DisplayName '${RULE_NAME}' |
    Where-Object { $_.Direction -eq 'Inbound' -and ($_.Enabled -eq 'True' -or $_.Enabled -eq $true) } |
    ForEach-Object { ($_ | Get-NetFirewallApplicationFilter).Program }
)
$cats = @(Get-NetConnectionProfile | ForEach-Object { $_.NetworkCategory.ToString() })
[pscustomobject]@{ programs = $programs; categories = $cats } | ConvertTo-Json -Compress
`

export async function status(): Promise<FirewallStatus> {
  const execPath = programPath()
  const base: FirewallStatus = {
    ruleExists: false,
    execPath,
    command: firewallCommand(),
    profile: 'unknown',
    checked: false
  }

  if (process.platform !== 'win32') {
    return {
      ...base,
      ruleExists: true,
      profile: 'private',
      checked: true
    }
  }

  try {
    const { stdout } = await run('powershell.exe', [...PS_ARGS, PROBE_SCRIPT])
    const parsed = JSON.parse(stdout.trim() || '{}') as Partial<ProbeResult>

    // ConvertTo-Json colapsa array de 1 item em escalar.
    const asList = (value: unknown): string[] =>
      Array.isArray(value) ? value.map(String) : value ? [String(value)] : []

    const categories = asList(parsed.categories)
    const programs = asList(parsed.programs)

    // Se qualquer rede ativa esta como Publica, avisamos: nesse perfil o
    // Windows bloqueia mesmo com a regra private criada.
    const profile: NetworkProfile = categories.includes('Public')
      ? 'public'
      : categories.includes('Private')
        ? 'private'
        : categories.includes('DomainAuthenticated')
          ? 'domain'
          : (PROFILE_MAP[categories[0] ?? ''] ?? 'unknown')

    // A regra so vale se apontar para ESTE executavel: no build portatil o
    // caminho de extracao muda entre versoes e a regra antiga fica orfa.
    // A comparacao ignora caixa porque o Windows guarda o caminho normalizado.
    const wanted = execPath.toLowerCase()
    const ruleExists = programs.some((program) => program.toLowerCase() === wanted)

    return { ...base, ruleExists, profile, checked: true }
  } catch (err) {
    console.warn('[firewall] checagem falhou:', (err as Error).message)
    return base
  }
}

/**
 * Cria a regra com elevacao (UAC).
 *
 * Passa por um .cmd temporario em vez de encadear aspas por
 * PowerShell -> cmd -> netsh: o caminho do exe tem espacos e o escape triplo
 * quebra de formas dificeis de diagnosticar.
 */
export async function authorize(): Promise<{ ok: boolean; error?: string }> {
  if (process.platform !== 'win32') {
    return { ok: false, error: 'Disponível apenas no Windows.' }
  }

  const scriptPath = join(app.getPath('temp'), `lan-screenshare-firewall-${Date.now()}.cmd`)
  const body = [
    '@echo off',
    `netsh advfirewall firewall delete rule name="${RULE_NAME}" >nul 2>&1`,
    `netsh advfirewall firewall add rule name="${RULE_NAME}" dir=in action=allow program="${programPath()}" enable=yes profile=any`,
    'exit /b %errorlevel%'
  ].join('\r\n')

  try {
    await writeFile(scriptPath, body, 'utf8')
    await run('powershell.exe', [
      ...PS_ARGS,
      `$p = Start-Process -FilePath '${scriptPath}' -Verb RunAs -Wait -WindowStyle Hidden -PassThru; exit $p.ExitCode`
    ])
    const after = await status()
    return after.ruleExists
      ? { ok: true }
      : { ok: false, error: 'A regra não apareceu depois do comando. Tente o modo manual.' }
  } catch (err) {
    const message = (err as Error).message
    // Cancelar o UAC cai aqui; vale mostrar um texto que faca sentido.
    const cancelled = /cancel|1223|operation was canceled/i.test(message)
    return {
      ok: false,
      error: cancelled
        ? 'Você cancelou a permissão do Windows. Sem ela, ninguém consegue se conectar.'
        : message
    }
  } finally {
    await unlink(scriptPath).catch(() => {})
  }
}
