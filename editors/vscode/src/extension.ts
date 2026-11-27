import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  Trace,
  TransportKind,
} from "vscode-languageclient/node";

const LANGUAGE_ID = "super";
const CLIENT_ID = "super-lsp";
const CLIENT_NAME = "Super Language Server";

let client: LanguageClient | undefined;

function resolveStdlibRoot(
  workspaceFolder?: vscode.WorkspaceFolder
): string | undefined {
  for (const key of ["SUPER_STDLIB", "SUPER_STD"]) {
    const fromEnv = process.env[key]?.trim();
    if (fromEnv && fs.existsSync(path.join(fromEnv, "mod.sr"))) {
      return fromEnv;
    }
  }

  const roots: string[] = [];
  if (workspaceFolder) {
    roots.push(workspaceFolder.uri.fsPath);
  }
  for (const f of vscode.workspace.workspaceFolders ?? []) {
    roots.push(f.uri.fsPath);
  }

  for (const root of roots) {
  for (const candidate of [
      path.join(root, "stdlib"),
      path.join(root, "..", "stdlib"),
    ]) {
      if (fs.existsSync(path.join(candidate, "mod.sr"))) {
        return path.resolve(candidate);
      }
    }
  }

  const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.HOME ?? "", ".cargo");
  const installed = path.join(cargoHome, "share", "super", "stdlib");
  if (fs.existsSync(path.join(installed, "mod.sr"))) {
    return installed;
  }

  return undefined;
}

function lspProcessEnv(
  workspaceFolder?: vscode.WorkspaceFolder
): NodeJS.ProcessEnv {
  const env = { ...process.env };
  const stdlib = resolveStdlibRoot(workspaceFolder);
  if (stdlib) {
    env.SUPER_STDLIB = stdlib;
    env.SUPER_STD = stdlib;
  }
  return env;
}

function resolveSuperBinary(
  workspaceFolder?: vscode.WorkspaceFolder
): string | undefined {
  const cfg = vscode.workspace.getConfiguration("super");
  const configured = cfg.get<string>("languageServerPath")?.trim();
  if (configured && fs.existsSync(configured)) {
    return configured;
  }

  for (const name of ["super-lsp", "super"]) {
    const which = process.env.PATH?.split(path.delimiter).flatMap((dir) => {
      const candidate = path.join(dir, name);
      return fs.existsSync(candidate) ? [candidate] : [];
    });
    if (which?.[0]) {
      return which[0];
    }
  }

  const roots: string[] = [];
  if (workspaceFolder) {
    roots.push(workspaceFolder.uri.fsPath);
  }
  const folders = vscode.workspace.workspaceFolders ?? [];
  for (const f of folders) {
    roots.push(f.uri.fsPath);
  }

  for (const root of roots) {
    for (const rel of [
      "target/release/super",
      "target/debug/super",
      path.join("crates", "super-cli", "target", "release", "super"),
    ]) {
      const candidate = path.join(root, rel);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }

  const cargoHome = process.env.CARGO_HOME ?? path.join(process.env.HOME ?? "", ".cargo");
  const cargoBin = path.join(cargoHome, "bin", "super");
  if (fs.existsSync(cargoBin)) {
    return cargoBin;
  }

  return undefined;
}

function startLanguageClient(context: vscode.ExtensionContext): void {
  const folder = vscode.workspace.workspaceFolders?.[0];
  const serverPath = resolveSuperBinary(folder);
  if (!serverPath) {
    void vscode.window.showWarningMessage(
      "Super: language server not found. Install `super` on PATH or set super.languageServerPath."
    );
    return;
  }

  const cfg = vscode.workspace.getConfiguration("super");
  const args = cfg.get<string[]>("languageServerArgs") ?? ["lsp"];
  const trace = cfg.get<string>("trace.server") ?? "off";

  const lspEnv = lspProcessEnv(folder);
  const serverOptions: ServerOptions = {
    run: {
      command: serverPath,
      args,
      transport: TransportKind.stdio,
      options: { env: lspEnv },
    },
    debug: {
      command: serverPath,
      args,
      transport: TransportKind.stdio,
      options: { env: lspEnv },
    },
  };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: LANGUAGE_ID }],
    synchronize: {
      fileEvents: vscode.workspace.createFileSystemWatcher("**/*.sr"),
    },
    outputChannelName: CLIENT_NAME,
    traceOutputChannel: vscode.window.createOutputChannel(`${CLIENT_NAME} Trace`),
  };

  client = new LanguageClient(CLIENT_ID, CLIENT_NAME, serverOptions, clientOptions);
  client.registerProposedFeatures();
  const traceLevel =
    trace === "verbose"
      ? Trace.Verbose
      : trace === "messages"
        ? Trace.Messages
        : Trace.Off;
  client.setTrace(traceLevel);
  void client.start();
  context.subscriptions.push({
    dispose: () => {
      void client?.stop();
    },
  });
}

class SuperTaskProvider implements vscode.TaskProvider {
  private static type = "super";

  provideTasks(): vscode.Task[] {
    const defs = [
      { task: "run", detail: "super run <file>" },
      { task: "build", detail: "super build --release <file>" },
      { task: "test", detail: "super test" },
      { task: "check", detail: "super check <file>" },
    ];
    return defs.map((d) => {
      const def = {
        type: SuperTaskProvider.type,
        task: d.task,
      };
      return new vscode.Task(def, vscode.TaskScope.Workspace, `Super: ${d.task}`, SuperTaskProvider.type);
    });
  }

  resolveTask(task: vscode.Task): vscode.Task | undefined {
    const raw = task.definition as { type?: string; task?: string; file?: string };
    if (raw?.type !== SuperTaskProvider.type || !raw.task) {
      return undefined;
    }
    const def: vscode.TaskDefinition = {
      type: SuperTaskProvider.type,
      task: raw.task,
      file: raw.file,
    };
    const file =
      def.file ??
      vscode.window.activeTextEditor?.document.uri.fsPath;
    const bin = resolveSuperBinary() ?? "super";
    const sub = raw.task;
    const args = buildCliArgs(sub, file);
    const exec = new vscode.ShellExecution(bin, args, { cwd: workspaceCwd() });
    const resolved = new vscode.Task(
      def,
      task.scope ?? vscode.TaskScope.Workspace,
      `Super: ${sub}`,
      SuperTaskProvider.type,
      exec
    );
    resolved.detail = args.join(" ");
    resolved.group =
      sub === "build" ? vscode.TaskGroup.Build : vscode.TaskGroup.Test;
    resolved.problemMatchers = [];
    return resolved;
  }
}

function workspaceCwd(): string | undefined {
  return vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
}

function buildCliArgs(sub: string, file?: string): string[] {
  switch (sub) {
    case "run":
      return file ? ["run", file] : ["run"];
    case "build":
      return file ? ["build", "--release", file] : ["build", "--release"];
    case "test":
      return ["test"];
    case "check":
      return file ? ["check", file] : ["check"];
    default:
      return [sub];
  }
}

async function runCommand(sub: string): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  const file = editor?.document.uri.fsPath;
  if (!file && sub !== "test") {
    void vscode.window.showErrorMessage("Super: open a .sr file first.");
    return;
  }
  const bin = resolveSuperBinary() ?? "super";
  const term = vscode.window.createTerminal("Super");
  term.show();
  term.sendText([bin, ...buildCliArgs(sub, file)].join(" "));
}

export function activate(context: vscode.ExtensionContext): void {
  startLanguageClient(context);
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider("super", new SuperTaskProvider())
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("super.runCurrentFile", () => runCommand("run"))
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("super.checkCurrentFile", () => runCommand("check"))
  );
}

export async function deactivate(): Promise<void> {
  if (!client) {
    return;
  }
  await client.stop();
}