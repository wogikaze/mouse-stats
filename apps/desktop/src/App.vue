<script setup lang="ts">
import { computed, defineAsyncComponent, onMounted, onBeforeUnmount, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  keyboard,
  localDay,
  summarize,
  type Snapshot,
  type Settings,
} from "@mouse-stats/contracts";
const Chart = defineAsyncComponent(() => import("./Chart.vue"));
import KeyCap from "./KeyCap.vue";
const native = "__TAURI_INTERNALS__" in window;
const nativeWindow = !!(
  window as Window & {
    __TAURI_INTERNALS__?: { metadata?: { currentWindow?: unknown } };
  }
).__TAURI_INTERNALS__?.metadata?.currentWindow;
const uiActive = ref(!nativeWindow);
const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
const empty: Snapshot = {
  device_id: "",
  recording_id: "",
  settings: {
    consent: false,
    paused: false,
    autostart: false,
    sync_enabled: false,
    upload_history: false,
    timezone: zone,
    layout: "US",
    cloud_url: "",
  },
  rows: [],
  devices: [],
  last_saved: null,
  last_sync: null,
  sync_status: "ローカルのみ",
  error: null,
  generation_conflict: false,
  signed_in: false,
};
const state = ref<Snapshot>(structuredClone(empty)),
  draft = ref<Settings>({ ...empty.settings }),
  tab = ref("dashboard"),
  period = ref("7"),
  device = ref(""),
  metric = ref<"keys" | "mouse">("keys"),
  message = ref(""),
  busy = ref(false),
  confirm = ref(""),
  confirmText = ref("");
const today = computed(() => localDay(Date.now() / 1000, draft.value.timezone || zone));
const from = ref(""),
  to = ref("");
function daysAgo(days: number) {
  const d = new Date(`${today.value}T12:00:00Z`);
  d.setUTCDate(d.getUTCDate() - days);
  return d.toISOString().slice(0, 10);
}
const range = computed(() =>
  period.value === "custom"
    ? [from.value, to.value]
    : period.value === "all"
      ? ["", "9999"]
      : [daysAgo(Number(period.value) - 1), today.value],
);
const stats = computed(() =>
  summarize(
    state.value.rows,
    draft.value.timezone || zone,
    range.value[0],
    range.value[1],
    device.value,
  ),
);
const hasSplitDayBucket = computed(() => {
  const tz = draft.value.timezone || zone;
  const [fromDay, toDay] = range.value;
  return state.value.rows.some((row) => {
    const firstDay = localDay(row.hour_start, tz);
    const lastDay = localDay(row.hour_start + 3599, tz);
    return (
      firstDay !== lastDay && [firstDay, lastDay].some((day) => day >= fromDay && day <= toDay)
    );
  });
});
const annual = computed(() =>
  summarize(
    state.value.rows,
    draft.value.timezone || zone,
    daysAgo(363),
    today.value,
    device.value,
  ),
);
const calendar = computed(() =>
  Array.from({ length: 364 }, (_, i) => {
    const day = daysAgo(363 - i);
    const d = annual.value.days.get(day);
    return { day, value: metric.value === "keys" ? (d?.keys ?? 0) : (d?.mouse ?? 0) / 1000 };
  }),
);
const calendarMax = computed(() => Math.max(1, ...calendar.value.map((d) => d.value)));
const totals = computed(() =>
  [1, 7, 30, 0].map((days) => {
    const s = summarize(
      state.value.rows,
      draft.value.timezone || zone,
      days ? daysAgo(days - 1) : "",
      "9999",
      device.value,
    );
    return {
      label: days === 1 ? "今日" : days === 7 ? "過去7日" : days === 30 ? "過去30日" : "全期間",
      keys: s.presses,
      mouse: s.mouse / 1000,
    };
  }),
);
const hourData = computed(() => {
  const list = Array.from(stats.value.hours).sort((a, b) => a[0] - b[0]);
  return {
    labels: list.map(([, v]) => v.label),
    values: list.map(([, v]) => (metric.value === "keys" ? v.keys : v.mouse / 1000)),
  };
});
const timeOfDay = computed(() => {
  const values = Array(24).fill(0) as number[];
  for (const [epoch, v] of stats.value.hours) {
    const h = Number(
      new Intl.DateTimeFormat("en-GB", {
        timeZone: draft.value.timezone || zone,
        hour: "2-digit",
        hourCycle: "h23",
      }).format(new Date(epoch * 1000)),
    );
    values[h]! += metric.value === "keys" ? v.keys : v.mouse / 1000;
  }
  return values;
});
const layout = computed(() => keyboard(draft.value.layout));
const navigation = [
  0x137, 70, 0x245, 0x152, 0x147, 0x149, 0x153, 0x14f, 0x151, 0x148, 0x14b, 0x150, 0x14d,
];
const mainKeys = computed(() =>
  layout.value.slice(0, 6).map((row) => row.filter((k) => !navigation.includes(k.code))),
);
const navKeys = computed(() =>
  navigation.map((code, i) => ({
    key: layout.value.flat().find((k) => k.code === code)!,
    col: i < 9 ? (i % 3) + 1 : i === 9 ? 2 : i - 9,
    row: i < 9 ? Math.floor(i / 3) + 1 : i === 9 ? 5 : 6,
  })),
);
const padPositions: Record<number, [number, number, number?, number?]> = {
  69: [1, 2],
  0x135: [2, 2],
  55: [3, 2],
  74: [4, 2],
  71: [1, 3],
  72: [2, 3],
  73: [3, 3],
  78: [4, 3, 1, 2],
  75: [1, 4],
  76: [2, 4],
  77: [3, 4],
  79: [1, 5],
  80: [2, 5],
  81: [3, 5],
  0x11c: [4, 5, 1, 2],
  82: [1, 6, 2],
  83: [3, 6],
};
const labels = computed(() => new Map(layout.value.flat().map((k) => [k.code, k.label])));
const ranking = computed(() => Array.from(stats.value.keys).sort((a, b) => b[1] - a[1]));
const maxKey = computed(() => Math.max(1, ...stats.value.keys.values()));
const format = (v: number) =>
  new Intl.NumberFormat("ja-JP", { maximumFractionDigits: 1 }).format(v);
const date = (v: number | null) =>
  v ? new Date(v * 1000).toLocaleString("ja-JP") : "まだありません";
let timer: ReturnType<typeof setInterval> | undefined;
let unlistenFocus: (() => void) | undefined;
let unlistenShown: (() => void) | undefined;
async function refresh(edit = false) {
  if (!native) return;
  try {
    state.value = await invoke<Snapshot>("snapshot");
    if (edit) {
      draft.value = { ...state.value.settings, timezone: state.value.settings.timezone || zone };
    }
  } catch (e) {
    message.value = String(e);
  }
}
async function action(command: string, args: Record<string, unknown> = {}): Promise<boolean> {
  busy.value = true;
  message.value = "";
  try {
    if (!native) throw new Error("Windowsアプリから操作してください");
    await invoke(command, args);
    await refresh(true);
    return true;
  } catch (e) {
    message.value = String(e);
    return false;
  } finally {
    busy.value = false;
  }
}
async function save() {
  try {
    new Intl.DateTimeFormat("ja-JP", { timeZone: draft.value.timezone });
  } catch {
    message.value = "有効なIANAタイムゾーンを指定してください";
    return;
  }
  await action("save_settings", { settings: draft.value });
}
async function consent() {
  draft.value.consent = true;
  await save();
}
async function togglePause() {
  draft.value.paused = !state.value.settings.paused;
  await save();
}
async function destroy() {
  if (confirmText.value !== (confirm.value === "device" ? "再登録" : "削除")) return;
  const completed =
    confirm.value === "device"
      ? await action("register_new_device")
      : confirm.value === "generation"
        ? await action("resolve_generation", { keepLocal: false })
        : await action("delete_local");
  if (!completed) return;
  confirm.value = "";
  confirmText.value = "";
}
function startRefresh(edit = false) {
  if (timer) return;
  uiActive.value = true;
  void refresh(edit);
  timer = setInterval(() => void refresh(), 5000);
}
function stopRefresh() {
  if (timer) clearInterval(timer);
  timer = undefined;
}
onMounted(async () => {
  if (!native || !nativeWindow) {
    void refresh(true);
    return;
  }
  const appWindow = getCurrentWindow();
  unlistenShown = await appWindow.listen("mouse-stats://shown", () => startRefresh(true));
  unlistenFocus = await appWindow.onFocusChanged(async ({ payload: focused }) => {
    if (focused) startRefresh(true);
    else {
      stopRefresh();
      if (!(await appWindow.isVisible())) uiActive.value = false;
    }
  });
  if ((await appWindow.isVisible()) || (await appWindow.isFocused())) startRefresh(true);
});
onBeforeUnmount(() => {
  stopRefresh();
  unlistenFocus?.();
  unlistenShown?.();
});
</script>
<template>
  <div class="shell">
    <aside>
      <a class="brand" href="#" @click.prevent="tab = 'dashboard'"
        ><span class="brand-icon">↗</span
        ><span
          >mouse<span class="brand-light">stats</span><small>YOUR INPUT, OVER TIME</small></span
        ></a
      >
      <div class="nav-caption">ワークスペース</div>
      <nav aria-label="メインメニュー">
        <button :class="{ active: tab === 'dashboard' }" @click="tab = 'dashboard'">
          ▦ <span>ダッシュボード</span></button
        ><button :class="{ active: tab === 'keyboard' }" @click="tab = 'keyboard'">
          ⌨ <span>キーボード分析</span></button
        ><button :class="{ active: tab === 'settings' }" @click="tab = 'settings'">
          ⚙ <span>設定・アカウント</span>
        </button>
      </nav>
      <div class="sidebar-bottom">
        <span :class="['dot', { paused: state.settings.paused || !state.settings.consent }]"></span
        >{{
          state.settings.consent && !state.settings.paused
            ? "バックグラウンド計測中"
            : "計測停止中"
        }}<small>集計データはこのPCに保存されます</small
        ><span class="version">Mouse Stats / v0.1.0</span>
      </div>
    </aside>
    <main>
      <header>
        <div class="breadcrumb">
          ワークスペース
          <span
            >/
            {{
              tab === "dashboard"
                ? "ダッシュボード"
                : tab === "keyboard"
                  ? "キーボード分析"
                  : "設定・アカウント"
            }}</span
          >
        </div>
        <div class="header-actions">
          <span class="badge">{{ state.sync_status }}</span
          ><button
            class="subtle"
            :disabled="busy || !native || !state.settings.consent"
            @click="togglePause"
          >
            {{ state.settings.paused ? "▶ 再開" : "Ⅱ 一時停止" }}
          </button>
        </div>
      </header>
      <div v-if="!native" class="notice">
        ブラウザプレビューです。入力計測・保存・設定はWindowsアプリで利用できます。
      </div>
      <div v-if="message || state.error" class="notice error" role="alert">
        {{ message || state.error }}
      </div>
      <div v-if="state.generation_conflict" class="notice error">
        <strong>クラウドの統計が削除されています。</strong>
        <p>旧データは再送しません。このPCの旧データを保持する場合も、ローカル専用になります。</p>
        <button @click="action('resolve_generation', { keepLocal: true })">
          旧データをPCに保持して再開</button
        ><button @click="confirm = 'generation'">旧データを削除して再開</button>
      </div>
      <div class="content">
        <div class="title-row">
          <div>
            <div class="eyebrow">{{ tab === "settings" ? "PREFERENCES" : "INPUT INSIGHTS" }}</div>
            <h1>
              {{
                tab === "dashboard"
                  ? "日々の操作を、ひと目で。"
                  : tab === "keyboard"
                    ? "キーボード分析"
                    : "設定・アカウント"
              }}
            </h1>
            <p class="muted">
              {{
                tab === "settings"
                  ? "計測も同期も、自分でコントロール。"
                  : "入力内容を残さず、あなたの操作の傾向を可視化します。"
              }}
            </p>
          </div>
          <span class="date-label">{{ today }}</span>
        </div>
        <template v-if="tab !== 'settings'">
          <div class="filters">
            <div class="segmented">
              <button
                v-for="p in [
                  { id: '1', label: '今日' },
                  { id: '7', label: '7日' },
                  { id: '30', label: '30日' },
                  { id: 'all', label: '全期間' },
                  { id: 'custom', label: '任意期間' },
                ]"
                :key="p.id"
                :class="{ selected: period === p.id }"
                @click="period = p.id"
              >
                {{ p.label }}
              </button>
            </div>
            <select v-model="device" aria-label="デバイス">
              <option value="">全デバイス</option>
              <option :value="state.device_id">このPC</option>
              <option
                v-for="d in state.devices.filter((d) => d.id !== state.device_id)"
                :key="d.id"
                :value="d.id"
              >
                {{ d.name }}{{ d.revoked_at ? "（失効）" : "" }}
              </option></select
            ><span class="tz">{{ draft.timezone }}</span>
          </div>
          <div v-if="period === 'custom'" class="custom-range">
            <input v-model="from" type="date" aria-label="開始日" /> →
            <input v-model="to" type="date" aria-label="終了日" /><span
              v-if="from > to"
              class="danger"
              >終了日が開始日より前です</span
            >
          </div>
          <div v-if="tab === 'dashboard' && hasSplitDayBucket" class="notice" role="status">
            <strong>日別集計には境界誤差があり得ます。</strong>
            <p>
              UTCの1時間枠がローカル日付をまたいでいます。保存値は枠の開始日に集約されるため、日別・カレンダー表示に偏りが出る可能性があります。時間枠別の値は保持されています。
            </p>
          </div>
          <template v-if="tab === 'dashboard'">
            <div class="cards">
              <article class="stat-card">
                <span>KEY PRESSES <span class="stat-icon">⌨</span></span>
                <div class="big">{{ format(stats.presses) }}<small>回</small></div>
                <p>物理キーの初回押下のみ</p>
              </article>
              <article class="stat-card">
                <span>MOUSE MOVEMENT <span class="stat-icon">↗</span></span>
                <div class="big">{{ format(stats.mouse / 1000) }}<small>counts</small></div>
                <p>Raw Input の相対移動量</p>
              </article>
              <article class="stat-card status-card">
                <span>RECORDING STATUS</span>
                <div class="status-text">
                  <span
                    :class="['dot', { paused: state.settings.paused || !state.settings.consent }]"
                  ></span
                  >{{ state.settings.consent && !state.settings.paused ? "計測中" : "停止中" }}
                </div>
                <p>最終保存 {{ date(state.last_saved) }}</p>
              </article>
            </div>
            <section class="panel">
              <div class="panel-heading">
                <div>
                  <h2>年間アクティビティ</h2>
                  <p>過去52週の日別合計</p>
                </div>
                <div class="segmented compact">
                  <button :class="{ selected: metric === 'keys' }" @click="metric = 'keys'">
                    キー入力</button
                  ><button :class="{ selected: metric === 'mouse' }" @click="metric = 'mouse'">
                    マウス
                  </button>
                </div>
              </div>
              <div class="calendar">
                <span
                  v-for="day in calendar"
                  :key="day.day"
                  :title="`${day.day}: ${format(day.value)} ${metric === 'keys' ? '回' : 'counts'}`"
                  :style="{
                    background: day.value
                      ? `rgba(96,211,181,${0.18 + (0.82 * day.value) / calendarMax})`
                      : '#202e37',
                  }"
                ></span>
              </div>
              <div class="calendar-caption">
                <span>{{ daysAgo(363) }} — {{ today }}</span
                ><span>少 <i></i><i></i><i></i><i></i> 多</span>
              </div>
            </section>
            <section class="panel">
              <div class="panel-heading">
                <div>
                  <h2>時間帯の傾向</h2>
                  <p>選択期間のローカル時間帯別合計</p>
                </div>
                <span class="badge">{{ metric === "keys" ? "キー入力数" : "raw counts" }}</span>
              </div>
              <Chart
                v-if="uiActive"
                :labels="Array.from({ length: 24 }, (_, i) => `${i}:00`)"
                :values="timeOfDay"
                :unit="metric === 'keys' ? '回' : 'counts'"
              />
            </section>
            <details class="panel">
              <summary>UTC時間枠ごとの詳細（DSTの重複時刻をオフセットで区別）</summary>
              <Chart
                v-if="uiActive"
                :labels="hourData.labels"
                :values="hourData.values"
                :unit="metric === 'keys' ? '回' : 'counts'"
              />
            </details>
            <section class="panel">
              <h2>期間別サマリー</h2>
              <table>
                <thead>
                  <tr>
                    <th>期間</th>
                    <th>キー入力</th>
                    <th>マウス移動</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="t in totals" :key="t.label">
                    <td>{{ t.label }}</td>
                    <td>{{ format(t.keys) }} 回</td>
                    <td>{{ format(t.mouse) }} counts</td>
                  </tr>
                </tbody>
              </table>
            </section>
          </template>
          <template v-else
            ><section class="panel">
              <div class="panel-heading">
                <div>
                  <h2>物理キーボードヒートマップ</h2>
                  <p>キー名は配列上の位置の目安です。入力文字は保存しません。</p>
                </div>
                <select v-model="draft.layout" aria-label="キーボード配列" @change="save">
                  <option>US</option>
                  <option>JIS</option>
                </select>
              </div>
              <div class="keyboard">
                <div class="keyboard-grid">
                  <div class="key-main">
                    <div v-for="(row, i) in mainKeys" :key="i" class="key-row">
                      <KeyCap
                        v-for="key in row"
                        :key="key.code"
                        :key-info="key"
                        :count="stats.keys.get(key.code) ?? 0"
                        :maximum="maxKey"
                      />
                    </div>
                  </div>
                  <div class="key-navigation">
                    <KeyCap
                      v-for="item in navKeys"
                      :key="item.key.code"
                      :key-info="item.key"
                      :count="stats.keys.get(item.key.code) ?? 0"
                      :maximum="maxKey"
                      :style="{ gridColumn: item.col, gridRow: item.row }"
                    />
                  </div>
                  <div class="key-pad">
                    <KeyCap
                      v-for="key in layout[6]"
                      :key="key.code"
                      :key-info="key"
                      :count="stats.keys.get(key.code) ?? 0"
                      :maximum="maxKey"
                      :style="{
                        gridColumn: `${padPositions[key.code]![0]} / span ${padPositions[key.code]![2] ?? 1}`,
                        gridRow: `${padPositions[key.code]![1]} / span ${padPositions[key.code]![3] ?? 1}`,
                      }"
                    />
                  </div>
                </div>
              </div>
            </section>
            <section class="panel">
              <h2>キーランキング</h2>
              <p v-if="!ranking.length" class="empty">この期間の記録はまだありません。</p>
              <div v-for="([key, value], i) in ranking" :key="key" class="ranking">
                <span>{{ i + 1 }}</span
                ><b>{{ labels.get(key) ?? (key === 0 ? "Unknown" : `0x${key.toString(16)}`) }}</b>
                <div class="rank-track">
                  <div :style="{ width: `${(value / maxKey) * 100}%` }"></div>
                </div>
                <span>{{ format(value) }} 回</span>
              </div>
            </section></template
          >
          <p class="footnote">
            0は停止・未接続・記録なしを含む場合があります。別PCの値は最終同期時点の情報です。
          </p>
        </template>
        <template v-else>
          <section class="panel">
            <h2>計測と表示</h2>
            <label class="setting"
              ><span>Windowsログイン時に起動<small>トレイでバックグラウンド起動します</small></span
              ><input v-model="draft.autostart" type="checkbox" /></label
            ><label class="setting"
              ><span>計測を一時停止</span><input v-model="draft.paused" type="checkbox" /></label
            ><label class="setting"
              ><span>タイムゾーン<small>IANA形式（例: Asia/Tokyo）</small></span
              ><input v-model="draft.timezone" placeholder="Asia/Tokyo" /></label
            ><label class="setting"
              ><span>キーボード配列</span
              ><select v-model="draft.layout" aria-label="キーボード配列">
                <option>US</option>
                <option>JIS</option>
              </select></label
            ><button class="primary" :disabled="busy" @click="save">設定を保存</button>
          </section>
          <section class="panel">
            <div class="panel-heading">
              <div>
                <h2>クラウド同期</h2>
                <p>任意・アカウントなしでもすべてのローカル統計を利用できます。</p>
              </div>
              <span class="badge">{{ state.signed_in ? "ログイン済み" : "未ログイン" }}</span>
            </div>
            <label class="setting"
              ><span>クラウドURL</span
              ><input
                v-model="draft.cloud_url"
                type="url"
                placeholder="https://your-app.workers.dev" /></label
            ><label class="setting"
              ><span
                >クラウド同期を有効にする<small
                  >時間別のキー回数・マウスcountsを送信します</small
                ></span
              ><input v-model="draft.sync_enabled" type="checkbox" /></label
            ><label class="setting"
              ><span
                >初回接続時に過去のローカル統計も送信する<small
                  >OFFの場合は接続後の新しい記録のみが対象です</small
                ></span
              ><input v-model="draft.upload_history" type="checkbox" :disabled="state.signed_in"
            /></label>
            <div class="button-row">
              <button class="primary" :disabled="busy" @click="save">同期設定を保存</button
              ><button :disabled="busy" @click="action('begin_login')">
                Passkeyで登録 / ログイン</button
              ><button :disabled="!state.signed_in || busy" @click="action('sync_now')">
                今すぐ同期</button
              ><button :disabled="!state.signed_in || busy" @click="action('logout')">
                ログアウト
              </button>
            </div>
            <p class="muted">最終同期 {{ date(state.last_sync) }}</p>
            <p class="muted">同じPCの別インストールを同時に動かすと重複計測されます。</p>
          </section>
          <section class="panel">
            <h2>デバイスとアカウント</h2>
            <p class="muted">
              Passkeyの追加・名前変更・失効、デバイス失効、リカバリーコード、統計削除・アカウント削除はブラウザで再認証して操作します。
            </p>
            <button @click="action('manage_account')">アカウント管理を開く ↗</button>
            <button :disabled="!state.signed_in || busy" @click="action('recover_device')">
              Windows Helloで端末から復旧
            </button>
            <p class="muted">
              Passkeyを失っても、ログイン済みPCの端末固有鍵とWindows
              Helloで本人確認できます。Helloが使えない場合は復旧コードを使ってください。
            </p>
            <button @click="confirm = 'device'">新しいPCとして再登録</button>
            <table v-if="state.devices.length">
              <thead>
                <tr>
                  <th>デバイス</th>
                  <th>最終接続</th>
                  <th>状態</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="d in state.devices" :key="d.id">
                  <td>{{ d.name }}{{ d.id === state.device_id ? "（このPC）" : "" }}</td>
                  <td>{{ date(d.last_seen) }}</td>
                  <td>{{ d.revoked_at ? "失効" : "有効" }}</td>
                </tr>
              </tbody>
            </table>
          </section>
          <section class="panel">
            <h2>プライバシーとデータ</h2>
            <p class="muted">
              入力文字・キーの押下順序・アプリ名・ウィンドウ名・クリップボードは保存しません。保存するのはUTCの1時間単位の集計値だけです。ウィンドウを閉じても計測は続きます。完全に終了するにはトレイの「終了」を選択してください。
            </p>
            <p class="muted">
              Passkey・ログイン済み端末・リカバリーコードをすべて失った場合、アカウントを復旧できません。
            </p>
            <button class="danger" @click="confirm = 'local'">このPCの統計を削除</button>
            <p class="muted">
              クラウド統計は残ります。クラウドの削除はアカウント管理から行ってください。
            </p>
          </section>
        </template>
      </div>
      <footer>
        プライバシーを大切にする入力統計 <span>最終保存 {{ date(state.last_saved) }}</span>
      </footer>
    </main>
    <div v-if="native && !state.settings.consent" class="modal-backdrop">
      <section class="modal" role="dialog" aria-modal="true" aria-labelledby="consent-title">
        <div class="eyebrow">WELCOME TO MOUSE STATS</div>
        <h2 id="consent-title">操作の傾向だけを記録します。</h2>
        <p>
          物理キーごとの押下回数とマウス相対移動量を1時間単位でこのPCに保存します。入力した文字・順序・使用アプリは記録しません。
        </p>
        <p>
          計測は画面を閉じても続き、トレイからいつでも停止できます。クラウド送信は初期状態ではOFFです。
        </p>
        <button class="primary" :disabled="busy" @click="consent">
          同意してローカル計測を開始
        </button>
      </section>
    </div>
    <div v-if="confirm" class="modal-backdrop">
      <section class="modal" role="dialog" aria-modal="true">
        <h2>
          {{ confirm === "device" ? "新しいPCとして再登録しますか？" : "統計を削除しますか？" }}
        </h2>
        <p>
          {{
            confirm === "device"
              ? "同期を停止し、新しい端末IDで再起動します。ローカル統計を保持します。「再登録」と入力してください。"
              : "この操作は取り消せません。「削除」と入力してください。"
          }}
        </p>
        <input
          v-model="confirmText"
          :placeholder="confirm === 'device' ? '再登録' : '削除'"
          aria-label="操作確認"
        />
        <div class="button-row">
          <button
            @click="
              confirm = '';
              confirmText = '';
            "
          >
            キャンセル</button
          ><button
            class="danger"
            :disabled="confirmText !== (confirm === 'device' ? '再登録' : '削除') || busy"
            @click="destroy"
          >
            {{ confirm === "device" ? "再登録して再起動" : "削除する" }}
          </button>
        </div>
      </section>
    </div>
  </div>
</template>
