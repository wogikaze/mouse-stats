import { startRegistration, startAuthentication } from "@simplewebauthn/browser";
import "./browser.css";
const el = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const input = (id: string) => el<HTMLInputElement>(id).value.trim();
const params = new URLSearchParams(location.search),
  mode = params.get("mode") === "device" ? "device" : "manage";
let access = "",
  busy = false,
  deletion = "";
const random = () =>
  btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32))))
    .replaceAll("+", "-")
    .replaceAll("/", "_")
    .replaceAll("=", "");
const state = params.get("state") ?? random(),
  pkce = params.get("pkce") ?? random();
function status(message: string) {
  el("status").textContent = message;
}
async function api(path: string, body?: unknown, method = body ? "POST" : "GET") {
  const response = await fetch(`/api/v1${path}`, {
    method,
    headers: {
      ...(body ? { "Content-Type": "application/json" } : {}),
      ...(access ? { Authorization: `Bearer ${access}` } : {}),
    },
    ...(body ? { body: JSON.stringify(body) } : {}),
  });
  const v = await response.json();
  if (!response.ok) throw Error(v.error ?? `HTTP ${response.status}`);
  return v;
}
async function guarded(fn: () => Promise<void>) {
  if (busy) return;
  busy = true;
  document.querySelectorAll("button").forEach((b) => (b.disabled = true));
  status("処理中…");
  try {
    await fn();
  } catch (e) {
    status(`操作できませんでした: ${e instanceof Error ? e.message : String(e)}`);
  } finally {
    busy = false;
    document.querySelectorAll("button").forEach((b) => (b.disabled = false));
  }
}
async function authenticate(kind: "login" | "register") {
  const options = await api(`/auth/${kind}/options`, {
    username: input("username"),
    mode,
    state,
    pkce,
  });
  const response =
    kind === "register"
      ? await startRegistration({ optionsJSON: options.options })
      : await startAuthentication({ optionsJSON: options.options });
  const result = await api(`/auth/${kind}/verify`, {
    challenge_id: options.challenge_id,
    response,
  });
  if (mode === "device") {
    if (result.state !== state) throw Error("STATE_MISMATCH");
    const link = el<HTMLAnchorElement>("callback");
    link.href = result.redirect;
    link.hidden = false;
    el("auth").hidden = true;
    status("認証が完了しました。Windowsアプリに戻って同期設定を確認してください。");
    location.href = result.redirect;
  } else {
    access = result.access_token;
    el("auth").hidden = true;
    el("account").hidden = false;
    await load();
    status("再認証しました。管理操作は5分間有効です。");
  }
}
async function load() {
  const [keys, devices] = await Promise.all([api("/passkeys"), api("/devices")]);
  el("passkeys").replaceChildren();
  el("devices").replaceChildren();
  for (const key of keys.passkeys) {
    const div = document.createElement("div");
    div.className = "item";
    const name = document.createElement("input");
    name.value = key.name;
    name.maxLength = 80;
    name.setAttribute("aria-label", "Passkeyの名前");
    div.append(name);
    const save = document.createElement("button");
    save.textContent = "名前を保存";
    save.onclick = () =>
      void guarded(async () => {
        await api(
          `/passkeys/${encodeURIComponent(key.credential_id)}`,
          { name: name.value },
          "PATCH",
        );
        status("名前を変更しました");
      });
    div.append(save);
    const revoke = document.createElement("button");
    revoke.textContent = key.revoked_at ? "失効済み" : "失効";
    revoke.disabled = !!key.revoked_at;
    revoke.onclick = () =>
      void guarded(async () => {
        await api(`/passkeys/${encodeURIComponent(key.credential_id)}`, undefined, "DELETE");
        await load();
        status("操作を反映しました。最後の有効なPasskeyは失効できません。");
      });
    div.append(revoke);
    el("passkeys").append(div);
  }
  for (const device of devices.devices) {
    const div = document.createElement("div");
    div.className = "item";
    const name = document.createElement("span");
    name.textContent = `${device.name} — ${device.revoked_at ? "失効済み" : new Date(device.last_seen * 1000).toLocaleString("ja-JP")}`;
    div.append(name);
    if (!device.revoked_at) {
      const button = document.createElement("button");
      button.textContent = "デバイスを失効";
      button.onclick = () =>
        void guarded(async () => {
          await api(`/devices/${device.id}/revoke`, {});
          await load();
          status("デバイスを失効しました");
        });
      div.append(button);
    }
    el("devices").append(div);
  }
}
el("login").onclick = () => void guarded(() => authenticate("login"));
el("register").onclick = () => void guarded(() => authenticate("register"));
el("recover").onclick = () =>
  void guarded(async () => {
    const v = await api("/auth/recover", {
      username: input("username"),
      code: input("recovery-code"),
    });
    el<HTMLInputElement>("recovery-code").value = "";
    access = v.access_token;
    el("auth").hidden = true;
    el("account").hidden = false;
    await load();
    status("コードで復旧しました。新しいPasskeyを追加してください。");
  });
el("add-key").onclick = () =>
  void guarded(async () => {
    const v = await api("/passkeys/options", {});
    const response = await startRegistration({ optionsJSON: v.options });
    await api("/passkeys", { challenge_id: v.challenge_id, name: input("key-name"), response });
    await load();
    status("Passkeyを追加しました");
  });
el("recovery").onclick = () =>
  void guarded(async () => {
    const v = await api("/recovery", {});
    el("codes").textContent = v.codes.join("\n");
    status("コードを安全な場所に保存してください。再表示できません。");
  });
for (const id of ["reset", "delete"])
  el(id).onclick = () => {
    deletion = id;
    el("confirmation").hidden = false;
    el("confirmation-title").textContent =
      id === "reset" ? "クラウド統計を削除" : "アカウントを削除";
    el<HTMLInputElement>("confirmation-input").value = "";
    el("confirmation-input").focus();
  };
el("cancel").onclick = () => {
  deletion = "";
  el("confirmation").hidden = true;
};
el("confirm-delete").onclick = () =>
  void guarded(async () => {
    if (input("confirmation-input") !== "削除") throw Error("「削除」と入力してください");
    if (deletion === "reset") {
      await api("/data/reset", {});
      status("統計を削除しました。各PCで旧データの扱いを確認してください。");
    } else {
      await api("/account", undefined, "DELETE");
      access = "";
      el("account").hidden = true;
      el("auth").hidden = false;
      status("アカウントを削除しました");
    }
    el("confirmation").hidden = true;
  });
el("logout").onclick = () =>
  void guarded(async () => {
    await api("/auth/logout", {});
    access = "";
    el("account").hidden = true;
    el("auth").hidden = false;
    el("codes").textContent = "";
    status("ログアウトしました");
  });
const recovery = params.get("device_recovery");
if (recovery) {
  history.replaceState(null, "", "/?mode=manage");
  void guarded(async () => {
    const result = await api("/auth/device-recovery/exchange", { code: recovery });
    access = result.access_token;
    el("auth").hidden = true;
    el("account").hidden = false;
    await load();
    status("端末固有鍵で確認しました。新しいPasskeyを追加してください。");
  });
}
