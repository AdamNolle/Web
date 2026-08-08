import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState } from 'react';
import { isTauri, transport } from './transport';
import {
  SettingsSchema,
  type ArchiveImportPlatform,
  type ArchiveImportResult,
  type Dashboard,
  type DigestItem,
  type EditionDetail,
  type FeedbackSignal,
  type LibraryItem,
  type MastodonProbeResult,
  type OpmlCandidate,
  type Settings,
  type SyncOutcome,
  type Source,
} from './types';

type View = 'today' | 'trends' | 'library' | 'sources' | 'activity' | 'settings';
type ThemeMode = 'auto' | 'light' | 'dark';
type ZoomScale = 1 | 1.25 | 1.5 | 2;
type PaletteCommand = {
  id: string;
  label: string;
  detail: string;
  view: View;
  targetId?: string;
};
type FeedReviewCandidate = OpmlCandidate & { reviewId: string };
type FeedImportResult = FeedReviewCandidate & { added: boolean };
type SourceDialog = { action: 'rename' | 'delete'; source: Source };

const reviewCandidates = (candidates: OpmlCandidate[]): FeedReviewCandidate[] =>
  candidates.map((candidate, index) => ({
    ...candidate,
    reviewId: `feed-${index}`,
  }));

const views: Array<{ id: View; label: string }> = [
  { id: 'today', label: 'Today' },
  { id: 'trends', label: 'Trends' },
  { id: 'library', label: 'Library' },
  { id: 'sources', label: 'Sources' },
  { id: 'activity', label: 'Activity' },
  { id: 'settings', label: 'Privacy & settings' },
];

const THEME_STORAGE_KEY = 'web.presentation.theme';
const ZOOM_STORAGE_KEY = 'web.presentation.zoom';
const ZOOM_SCALES: readonly ZoomScale[] = [1, 1.25, 1.5, 2];

const formatDate = (value: string | null) => {
  if (value === null) return 'Not yet';
  if (value === 'Not connected' || value === 'Not yet') return value;
  const date = new Date(value);
  if (Number.isNaN(date.valueOf())) return 'Time unavailable';
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(
    date,
  );
};

const timestampLabel = (kind: 'published' | 'updated' | 'fetched') =>
  kind === 'published' ? 'Published' : kind === 'updated' ? 'Updated' : 'Retrieved';

const requestId = () => crypto.randomUUID();
const STATE_CHECK_MS = navigator.userAgent.includes('jsdom') ? 250 : 30_000;
const isThemeMode = (value: string | null): value is ThemeMode =>
  value === 'auto' || value === 'light' || value === 'dark';
const isZoomScale = (value: string | null): value is `${ZoomScale}` =>
  value === '1' || value === '1.25' || value === '1.5' || value === '2';
const initialThemeMode = (): ThemeMode => {
  try {
    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    return isThemeMode(saved) ? saved : 'auto';
  } catch {
    return 'auto';
  }
};
const initialZoomScale = (): ZoomScale => {
  try {
    const saved = window.localStorage.getItem(ZOOM_STORAGE_KEY);
    return isZoomScale(saved) ? (Number(saved) as ZoomScale) : 1;
  } catch {
    return 1;
  }
};
const statusLabel = (value: string) => value.replaceAll('_', ' ');
const MAX_OPENABLE_URL_BYTES = 2 * 1024;
type PlatformFamily = 'apple' | 'windows' | 'linux' | 'other';

// Client Hints are the browser's current-platform signal. Fall back only when they are not
// available (for example, Firefox); a user-agent string may be spoofed and must not override it.
const platformFamily = (): PlatformFamily => {
  const navigatorWithClientHints = navigator as Navigator & {
    userAgentData?: { platform?: string };
  };
  const signal =
    navigatorWithClientHints.userAgentData?.platform || navigator.platform || navigator.userAgent;
  if (/mac|iphone|ipad|ipod/i.test(signal)) return 'apple';
  if (/windows|win32|win64/i.test(signal)) return 'windows';
  if (/linux|x11/i.test(signal)) return 'linux';
  return 'other';
};

const isApplePlatform = () => {
  return platformFamily() === 'apple';
};
const commandShortcutLabel = () => (isApplePlatform() ? '⌘ K' : 'Ctrl K');
const zoomShortcutLabel = () => (isApplePlatform() ? '⌘ + / −' : 'Ctrl + / −');
const ollamaSetupHint = () => {
  switch (platformFamily()) {
    case 'apple':
      return 'On macOS, install and start Ollama yourself, then choose one of its locally installed models here.';
    case 'linux':
      return 'On Linux, install and start Ollama through your approved local setup, then choose one of its locally installed models here.';
    default:
      return 'On Windows, install and start Ollama yourself, then choose one of its locally installed models here.';
  }
};
const isCommandShortcut = (event: KeyboardEvent) =>
  (isApplePlatform() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey) &&
  !event.altKey &&
  !event.shiftKey &&
  event.key.toLowerCase() === 'k';
const canOpenOriginal = (value: string | null): value is string => {
  if (!value || new TextEncoder().encode(value).byteLength > MAX_OPENABLE_URL_BYTES) return false;
  try {
    const url = new URL(value);
    return (
      url.protocol === 'https:' &&
      Boolean(url.hostname) &&
      url.username.length === 0 &&
      url.password.length === 0
    );
  } catch {
    return false;
  }
};

const keepOpenEdition = (displayed: Dashboard, fresh: Dashboard): Dashboard => {
  const privacyChanged = fresh.privacyEpoch !== displayed.privacyEpoch;
  if (!privacyChanged) {
    return {
      ...fresh,
      edition: displayed.edition,
      items: displayed.items,
      trends: displayed.trends,
    };
  }
  const validSources = new Set(fresh.sources.map((source) => source.id));
  const freshItems = new Map(fresh.items.map((item) => [item.id, item]));
  const items = displayed.items
    .filter((item) => validSources.has(item.sourceId) && freshItems.has(item.id))
    .map((item) => freshItems.get(item.id) ?? item);
  const itemIds = new Set(items.map((item) => item.id));
  const freshTrends = new Map(fresh.trends.map((trend) => [trend.id, trend]));
  const trends = displayed.trends
    .filter(
      (trend) => freshTrends.has(trend.id) && trend.evidenceIds.every((id) => itemIds.has(id)),
    )
    .map((trend) => freshTrends.get(trend.id) ?? trend);
  return { ...fresh, edition: displayed.edition, items, trends };
};

const safeErrorMessage = (error: unknown, fallback: string) => {
  if (typeof error === 'object' && error && 'message' in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === 'string' && message.length <= 300) return message;
  }
  return fallback;
};

function App() {
  const [dashboard, setDashboard] = useState<Dashboard>();
  const [view, setView] = useState<View>('today');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState('Loading your local edition…');
  const [loadFailed, setLoadFailed] = useState(false);
  const [operationFailed, setOperationFailed] = useState(false);
  const [operationErrorTarget, setOperationErrorTarget] = useState<'rss-url'>();
  const [undoId, setUndoId] = useState<string>();
  const [pendingDashboard, setPendingDashboard] = useState<Dashboard>();
  const [syncReport, setSyncReport] = useState<SyncOutcome>();
  const [themeMode, setThemeMode] = useState<ThemeMode>(initialThemeMode);
  const [zoomScale, setZoomScale] = useState<ZoomScale>(initialZoomScale);
  const [paletteOpen, setPaletteOpen] = useState(false);
  const [paletteQuery, setPaletteQuery] = useState('');
  const [activeCommandIndex, setActiveCommandIndex] = useState(0);
  const mainRef = useRef<HTMLElement>(null);
  const dashboardRef = useRef<Dashboard | undefined>(undefined);
  const pendingDashboardRef = useRef<Dashboard | undefined>(undefined);
  const highestPrivacyEpochRef = useRef(0);
  const mutationGenerationRef = useRef(0);
  const undoButtonRef = useRef<HTMLButtonElement>(null);
  const feedbackOriginRef = useRef<{ itemId: string; signal: FeedbackSignal } | undefined>(
    undefined,
  );
  const paletteDialogRef = useRef<HTMLDivElement>(null);
  const paletteInputRef = useRef<HTMLInputElement>(null);
  const paletteReturnFocusRef = useRef<HTMLElement | null>(null);

  useLayoutEffect(() => {
    const media =
      typeof window.matchMedia === 'function'
        ? window.matchMedia('(prefers-color-scheme: dark)')
        : undefined;
    const applyTheme = () => {
      document.documentElement.dataset.theme =
        themeMode === 'auto' ? (media?.matches ? 'dark' : 'light') : themeMode;
      document.documentElement.dataset.themeMode = themeMode;
    };
    applyTheme();
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, themeMode);
    } catch {
      // Presentation preferences are optional when storage is unavailable.
    }
    if (themeMode !== 'auto' || !media) return;
    media.addEventListener('change', applyTheme);
    return () => media.removeEventListener('change', applyTheme);
  }, [themeMode]);

  useEffect(() => {
    try {
      window.localStorage.setItem(ZOOM_STORAGE_KEY, String(zoomScale));
    } catch {
      // Reading-scale preferences are optional when storage is unavailable.
    }
    if (!isTauri()) return;
    void import('@tauri-apps/api/webview')
      .then(({ getCurrentWebview }) => getCurrentWebview().setZoom(zoomScale))
      .catch(() => {
        // The visual preference stays local even if a host cannot apply it yet.
      });
  }, [zoomScale]);

  const openCommandPalette = useCallback(() => {
    if (!dashboardRef.current) return;
    paletteReturnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setPaletteQuery('');
    setActiveCommandIndex(0);
    setPaletteOpen(true);
  }, []);

  const closeCommandPalette = useCallback((restoreFocus = true) => {
    setPaletteOpen(false);
    setPaletteQuery('');
    setActiveCommandIndex(0);
    const returnTarget = paletteReturnFocusRef.current;
    paletteReturnFocusRef.current = null;
    if (restoreFocus) {
      window.requestAnimationFrame(() => returnTarget?.focus());
    }
  }, []);

  const acceptsDashboard = useCallback((fresh: Dashboard) => {
    const installedEpoch = Math.max(
      highestPrivacyEpochRef.current,
      dashboardRef.current?.privacyEpoch ?? 0,
      pendingDashboardRef.current?.privacyEpoch ?? 0,
    );
    if (fresh.privacyEpoch < installedEpoch) return false;
    highestPrivacyEpochRef.current = Math.max(installedEpoch, fresh.privacyEpoch);
    return true;
  }, []);

  const loadDashboard = useCallback(async () => {
    setLoadFailed(false);
    setNotice('Loading your local edition…');
    try {
      const fresh = await transport.getDashboard();
      if (acceptsDashboard(fresh)) {
        dashboardRef.current = fresh;
        setDashboard(fresh);
        setNotice(fresh.sources.length === 0 ? 'Ready to add your first source' : 'Edition ready');
      }
    } catch (error) {
      setLoadFailed(true);
      setNotice(
        safeErrorMessage(
          error,
          'Could not load the local edition. Your source data has not been changed.',
        ),
      );
    }
  }, [acceptsDashboard]);

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  useEffect(() => {
    if (!dashboard) return;
    const check = window.setInterval(() => {
      if (busy || document.visibilityState === 'hidden') return;
      const mutationGeneration = mutationGenerationRef.current;
      void transport
        .getDashboard()
        .then((fresh) => {
          // A deliberate mutation supersedes every poll that began before it. The privacy epoch is
          // independently monotonic across windows/processes, so a lower-epoch response is never
          // installed even if transport responses arrive out of order.
          if (mutationGeneration !== mutationGenerationRef.current || !acceptsDashboard(fresh))
            return;
          const current = dashboardRef.current;
          if (!current) {
            dashboardRef.current = fresh;
            setDashboard(fresh);
          } else if (fresh.edition.id !== current.edition.id) {
            pendingDashboardRef.current = fresh;
            setPendingDashboard(fresh);
            const kept = keepOpenEdition(current, fresh);
            dashboardRef.current = kept;
            setDashboard(kept);
            setNotice('A new local edition is available. Apply it when you are ready.');
          } else {
            dashboardRef.current = fresh;
            setDashboard(fresh);
          }
        })
        .catch(() => {
          // Background state checks stay quiet; deliberate actions continue to surface errors.
        });
    }, STATE_CHECK_MS);
    return () => window.clearInterval(check);
  }, [acceptsDashboard, busy, dashboard]);

  useEffect(() => {
    mainRef.current?.querySelector<HTMLElement>('h1')?.focus();
  }, [view]);

  useEffect(() => {
    if (undoId) undoButtonRef.current?.focus();
  }, [undoId]);

  useEffect(() => {
    const handleShortcut = (event: KeyboardEvent) => {
      if (isCommandShortcut(event)) {
        event.preventDefault();
        if (event.repeat) return;
        if (paletteOpen) closeCommandPalette();
        else openCommandPalette();
        return;
      }
      if (event.key === 'Escape' && paletteOpen) {
        event.preventDefault();
        closeCommandPalette();
      }
    };
    document.addEventListener('keydown', handleShortcut);
    return () => document.removeEventListener('keydown', handleShortcut);
  }, [closeCommandPalette, openCommandPalette, paletteOpen]);

  useEffect(() => {
    if (!paletteOpen) return;
    const frame = window.requestAnimationFrame(() => paletteInputRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [paletteOpen]);

  const perform = async (
    label: string,
    action: () => Promise<Dashboard>,
    errorTarget?: 'rss-url',
  ): Promise<boolean> => {
    if (busy) return false;
    mutationGenerationRef.current += 1;
    setBusy(true);
    setOperationFailed(false);
    setOperationErrorTarget(undefined);
    setNotice(label);
    try {
      const fresh = await action();
      if (acceptsDashboard(fresh)) {
        const current = dashboardRef.current;
        const pending = pendingDashboardRef.current;
        const next = current && pending ? keepOpenEdition(current, fresh) : fresh;
        dashboardRef.current = next;
        setDashboard(next);
        const nextPending = pending ? fresh : undefined;
        pendingDashboardRef.current = nextPending;
        setPendingDashboard(nextPending);
      }
      setNotice(`${label} complete`);
      return true;
    } catch (error) {
      setOperationFailed(true);
      setOperationErrorTarget(errorTarget);
      setNotice(safeErrorMessage(error, `${label} failed safely. Try again when you are ready.`));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const performSync = async (): Promise<boolean> => {
    if (busy) return false;
    mutationGenerationRef.current += 1;
    setBusy(true);
    setOperationFailed(false);
    setOperationErrorTarget(undefined);
    setSyncReport(undefined);
    setNotice('Deliberately synchronizing all bounded sources, including retry overrides');
    try {
      const result = await transport.syncSources(requestId());
      if (acceptsDashboard(result.dashboard)) {
        dashboardRef.current = result.dashboard;
        pendingDashboardRef.current = undefined;
        setDashboard(result.dashboard);
        setPendingDashboard(undefined);
      }
      setSyncReport(result.outcome);
      const partial = result.outcome.finality !== 'complete';
      setNotice(
        result.outcome.finality === 'unknown'
          ? 'Synchronization reached its bounded deadline. Some source effects may have committed; this request is sealed and will not replay.'
          : partial
            ? `Synchronization completed partially: ${result.outcome.changedSources} changed, ${result.outcome.unchangedSources} unchanged, ${result.outcome.failedSources} failed.`
            : `Synchronization complete: ${result.outcome.changedSources} changed and ${result.outcome.unchangedSources} unchanged.`,
      );
      return true;
    } catch (error) {
      setOperationFailed(true);
      setNotice(
        safeErrorMessage(error, 'Synchronization failed safely. Your prior edition is unchanged.'),
      );
      return false;
    } finally {
      setBusy(false);
    }
  };

  const probeMastodon = async (instanceUrl: string): Promise<MastodonProbeResult> => {
    if (busy) throw new Error('Another local operation is still running.');
    setBusy(true);
    setOperationFailed(false);
    setOperationErrorTarget(undefined);
    setNotice('Checking Mastodon OAuth compatibility without connecting an account');
    try {
      const result = await transport.probeMastodonInstance(instanceUrl);
      setNotice('Mastodon metadata is compatible. No account has been connected or stored.');
      return result;
    } catch (error) {
      setOperationFailed(true);
      setNotice(
        safeErrorMessage(error, 'Mastodon compatibility could not be checked. No account changed.'),
      );
      throw error;
    } finally {
      setBusy(false);
    }
  };

  const connectMastodon = async (label: string, instanceUrl: string): Promise<boolean> =>
    perform('Opening your browser for read-only Mastodon authorization', () =>
      transport.connectMastodon(requestId(), label, instanceUrl),
    );

  const closeUndo = () => {
    setUndoId(undefined);
    window.requestAnimationFrame(() => {
      const origin = feedbackOriginRef.current;
      const target = origin
        ? document.querySelector<HTMLButtonElement>(
            `[data-feedback-item="${origin.itemId}"][data-feedback-signal="${origin.signal}"]`,
          )
        : null;
      (target ?? mainRef.current?.querySelector<HTMLElement>('h1'))?.focus();
    });
  };

  const sendFeedback = async (itemId: string, signal: FeedbackSignal) => {
    feedbackOriginRef.current = { itemId, signal };
    const id = requestId();
    const saved = await perform('Saving feedback', () =>
      transport.recordFeedback(id, itemId, signal),
    );
    if (saved) setUndoId(id);
  };

  const openOriginal = async (url: string) => {
    if (busy) return;
    setBusy(true);
    setOperationFailed(false);
    setOperationErrorTarget(undefined);
    setNotice('Opening original source…');
    try {
      await transport.openOriginal(url);
      setNotice('Original source opened in your default browser.');
    } catch (error) {
      setOperationFailed(true);
      setNotice(
        safeErrorMessage(
          error,
          'The original source could not be opened safely. You can still copy its URL.',
        ),
      );
    } finally {
      setBusy(false);
    }
  };

  const importArchive = async (
    platform: ArchiveImportPlatform,
    label: string,
  ): Promise<boolean> => {
    let result: ArchiveImportResult | undefined;
    const platformLabel = platform === 'x' ? 'X' : 'Instagram';
    const finished = await perform(`Opening the ${platformLabel} archive picker`, async () => {
      result = await transport.importArchive(requestId(), platform, label);
      return result.dashboard;
    });
    if (!finished || !result) return false;

    if (result.status === 'canceled') {
      setNotice('Archive import canceled. No local data changed.');
      return false;
    }
    if (result.status === 'replayed') {
      setNotice('That archive import was already completed; no data was imported twice.');
      return true;
    }

    const skipped =
      result.skippedItems > 0
        ? ` ${result.skippedItems} invalid or exact duplicate entries were skipped.`
        : '';
    setNotice(
      `Imported ${result.importedItems} ${platformLabel} posts; ${result.changedItems} local items changed.${skipped}`,
    );
    return true;
  };

  const openSourcesControl = (controlId: 'rss-label' | 'archive-platform') => {
    setView('sources');
    window.requestAnimationFrame(() => document.getElementById(controlId)?.focus());
  };
  const openSourceCard = (sourceId: string) => {
    setView('sources');
    window.requestAnimationFrame(() => document.getElementById(`source-card-${sourceId}`)?.focus());
  };

  const paletteCommands: PaletteCommand[] = [
    ...views.map(({ id, label }) => ({
      id: `view-${id}`,
      label,
      detail: 'View',
      view: id,
    })),
    ...(dashboard?.sources ?? []).map((source) => ({
      id: `source-${source.id}`,
      label: source.label,
      detail: `Source · ${statusLabel(source.status)}`,
      view: 'sources' as const,
      targetId: `source-card-${source.id}`,
    })),
    ...(dashboard?.trends ?? []).map((trend) => ({
      id: `trend-${trend.id}`,
      label: trend.label,
      detail: `Trend · ${trend.sourceCount} independent sources`,
      view: 'trends' as const,
      targetId: `trend-card-${trend.id}`,
    })),
  ];
  const normalizedPaletteQuery = paletteQuery.trim().toLocaleLowerCase();
  const filteredPaletteCommands = paletteCommands.filter((command) =>
    `${command.label} ${command.detail}`.toLocaleLowerCase().includes(normalizedPaletteQuery),
  );
  const selectedCommandIndex = Math.min(
    activeCommandIndex,
    Math.max(filteredPaletteCommands.length - 1, 0),
  );

  const activatePaletteCommand = (command: PaletteCommand) => {
    closeCommandPalette(false);
    setView(command.view);
    window.requestAnimationFrame(() => {
      const target = command.targetId ? document.getElementById(command.targetId) : null;
      (target ?? mainRef.current?.querySelector<HTMLElement>('h1'))?.focus();
    });
  };

  const handlePaletteKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      setActiveCommandIndex((current) =>
        filteredPaletteCommands.length === 0 ? 0 : (current + 1) % filteredPaletteCommands.length,
      );
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      setActiveCommandIndex((current) =>
        filteredPaletteCommands.length === 0
          ? 0
          : (current - 1 + filteredPaletteCommands.length) % filteredPaletteCommands.length,
      );
    } else if (event.key === 'Enter') {
      event.preventDefault();
      const command = filteredPaletteCommands[selectedCommandIndex];
      if (command) activatePaletteCommand(command);
    }
  };

  const trapPaletteFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab') return;
    const focusable = paletteDialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), [tabindex]:not([tabindex="-1"])',
    );
    if (!focusable || focusable.length === 0) return;
    const first = focusable[0]!;
    const last = focusable[focusable.length - 1]!;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last?.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first?.focus();
    }
  };

  if (!dashboard) {
    return (
      <>
        <a className="skip-link" href="#main-content">
          Skip to content
        </a>
        <main id="main-content" className="loading" aria-live="polite" tabIndex={-1}>
          <div className="brand-mark" aria-hidden="true" />
          <p>{notice}</p>
          {loadFailed && (
            <button className="primary" onClick={() => void loadDashboard()}>
              Retry local load
            </button>
          )}
        </main>
      </>
    );
  }

  return (
    <div className="app-shell">
      <a className="skip-link" href="#main-content">
        Skip to content
      </a>
      <aside className="sidebar" aria-label="Primary navigation">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true" />
          <span>Web</span>
        </div>
        <p className="brand-subtitle">
          Your internet,
          <br />
          once or twice a day.
        </p>
        <nav>
          {views.map(({ id, label }) => (
            <button
              key={id}
              className={view === id ? 'nav-item active' : 'nav-item'}
              aria-current={view === id ? 'page' : undefined}
              onClick={() => setView(id)}
            >
              {label}
            </button>
          ))}
        </nav>
        <button className="command-trigger" type="button" onClick={openCommandPalette}>
          <span>Search &amp; commands</span>
          <kbd>{commandShortcutLabel()}</kbd>
        </button>
        <fieldset className="theme-control" role="radiogroup" aria-label="Theme">
          <legend>Appearance</legend>
          {(['auto', 'light', 'dark'] as const).map((mode) => (
            <label key={mode}>
              <input
                type="radio"
                name="theme-mode"
                value={mode}
                checked={themeMode === mode}
                onChange={() => setThemeMode(mode)}
              />
              <span>{mode === 'auto' ? 'Auto' : mode === 'light' ? 'Light' : 'Dark'}</span>
            </label>
          ))}
        </fieldset>
        <div className="privacy-badge">
          <span className="status-dot healthy" />
          Local-only inference
          <br />
          <small>No telemetry</small>
        </div>
      </aside>

      <main ref={mainRef} id="main-content" className="main-content" tabIndex={-1}>
        <div className="live-region" aria-live="polite" aria-atomic="true">
          {notice}
        </div>
        <div
          className={operationFailed ? 'visible-status error' : 'visible-status'}
          aria-hidden="true"
        >
          {busy ? `In progress: ${notice}` : `Status: ${notice}`}
        </div>
        {pendingDashboard && (
          <div className="operation-report" role="status">
            <span>New local edition available; your open edition has not been reordered.</span>
            <button
              className="secondary"
              onClick={() => {
                dashboardRef.current = pendingDashboard;
                pendingDashboardRef.current = undefined;
                setDashboard(pendingDashboard);
                setPendingDashboard(undefined);
                setNotice('New local edition applied');
                window.requestAnimationFrame(() => {
                  mainRef.current?.querySelector<HTMLElement>('h1')?.focus();
                });
              }}
            >
              Apply new edition
            </button>
          </div>
        )}
        {syncReport && syncReport.finality !== 'complete' && (
          <div className="operation-report" role="status">
            <span>
              {syncReport.finality === 'unknown'
                ? 'Bounded sync outcome is unknown; committed sources were retained and this request cannot replay.'
                : syncReport.failedSources > 0
                  ? 'Partial deliberate sync. Successful sources were kept; failed sources follow bounded retry timing.'
                  : 'The deliberate source cap was reached. Unattempted sources remain eligible; none are described as failed.'}
            </span>
            <button className="secondary" onClick={() => setView('sources')}>
              Review sources
            </button>
            <button className="secondary" onClick={() => setView('activity')}>
              Review activity
            </button>
          </div>
        )}
        {view === 'today' && (
          <Today
            dashboard={dashboard}
            busy={busy}
            onRefresh={() =>
              perform('Creating an edition from stored items', () =>
                transport.runDigest(requestId()),
              )
            }
            onSync={() => void performSync()}
            onFeedback={sendFeedback}
            onOpenOriginal={(url) => void openOriginal(url)}
            onSave={(itemId, saved) =>
              perform(saved ? 'Saving for later' : 'Removing from saved', () =>
                transport.setSaved(requestId(), itemId, saved),
              )
            }
            onAddSource={() => openSourcesControl('rss-label')}
            onImportArchive={() => openSourcesControl('archive-platform')}
          />
        )}
        {view === 'trends' && <Trends dashboard={dashboard} />}
        {view === 'library' && (
          <Library
            dashboard={dashboard}
            busy={busy}
            onSave={(itemId, saved) =>
              perform(saved ? 'Saving for later' : 'Removing from saved', () =>
                transport.setSaved(requestId(), itemId, saved),
              )
            }
            onOpenOriginal={(url) => void openOriginal(url)}
            onViewSource={openSourceCard}
            onExportSaved={async () => {
              if (busy) return;
              setBusy(true);
              setOperationFailed(false);
              setOperationErrorTarget(undefined);
              setNotice('Choosing a saved-item export location');
              try {
                const exported = await transport.exportSavedItems();
                setNotice(
                  exported
                    ? 'Saved items exported as a portable JSON file.'
                    : 'Saved item export canceled. No local data changed.',
                );
              } catch (error) {
                setOperationFailed(true);
                setNotice(safeErrorMessage(error, 'Saved items could not be exported safely.'));
              } finally {
                setBusy(false);
              }
            }}
          />
        )}
        {view === 'sources' && (
          <Sources
            dashboard={dashboard}
            busy={busy}
            onAdd={(label, url) =>
              perform(
                `Adding ${label}`,
                () => transport.addRssSource(requestId(), label, url),
                'rss-url',
              )
            }
            onImport={importArchive}
            onRename={(sourceId, label) =>
              perform(`Renaming ${label}`, () =>
                transport.renameSource(requestId(), sourceId, label),
              )
            }
            onSetPaused={(sourceId, paused) =>
              perform(paused ? 'Pausing source' : 'Resuming source', () =>
                transport.setSourcePaused(requestId(), sourceId, paused),
              )
            }
            onSyncSource={(sourceId) =>
              perform('Synchronizing this source', () =>
                transport.syncSource(requestId(), sourceId),
              )
            }
            onPickOpml={() => transport.pickOpml()}
            onDiscover={(url) => transport.discoverFeeds(url)}
            onProbeMastodon={probeMastodon}
            onConnectMastodon={connectMastodon}
            statusMessage={notice}
            statusIsError={operationFailed && operationErrorTarget === 'rss-url'}
            onDelete={(source) =>
              perform(`Deleting ${source.label} and its local data`, () =>
                transport.deleteSource(requestId(), source.id),
              )
            }
          />
        )}
        {view === 'activity' && <Activity dashboard={dashboard} />}
        {view === 'settings' && (
          <SettingsView
            dashboard={dashboard}
            busy={busy}
            zoomScale={zoomScale}
            onZoomScaleChange={setZoomScale}
            onSave={(settings) =>
              perform('Saving private settings', () =>
                transport.updateSettings(requestId(), settings),
              )
            }
            onReset={() =>
              perform('Resetting local learning', () => transport.resetLearning(requestId())).then(
                (reset) => {
                  if (reset) {
                    setUndoId(undefined);
                    feedbackOriginRef.current = undefined;
                  }
                  return reset;
                },
              )
            }
            onExport={() =>
              perform('Choosing a local backup location', () =>
                transport.exportBackup().then(() => dashboardRef.current ?? dashboard),
              )
            }
            onExportOpml={() =>
              perform('Choosing an OPML export location', () =>
                transport.exportOpml().then(() => dashboardRef.current ?? dashboard),
              )
            }
            onRestore={() =>
              perform('Choosing and validating a local backup', () =>
                transport.restoreBackup(requestId()),
              )
            }
          />
        )}
      </main>

      {paletteOpen && (
        <div
          className="command-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeCommandPalette();
          }}
        >
          <div
            ref={paletteDialogRef}
            className="command-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="command-palette-title"
            onKeyDown={trapPaletteFocus}
          >
            <div className="command-dialog-header">
              <div>
                <p className="eyebrow">Quick navigation</p>
                <h2 id="command-palette-title">Command palette</h2>
              </div>
              <button
                className="secondary command-close"
                type="button"
                onClick={() => closeCommandPalette()}
              >
                Close
              </button>
            </div>
            <label className="command-search" htmlFor="command-palette-input">
              <span className="sr-only">Search commands</span>
              <input
                ref={paletteInputRef}
                id="command-palette-input"
                role="combobox"
                aria-label="Search commands"
                aria-autocomplete="list"
                aria-expanded="true"
                aria-controls="command-palette-results"
                aria-activedescendant={
                  filteredPaletteCommands.length > 0
                    ? `palette-option-${filteredPaletteCommands[selectedCommandIndex]?.id}`
                    : undefined
                }
                value={paletteQuery}
                placeholder="Search views, sources, and trends"
                onChange={(event) => {
                  setPaletteQuery(event.target.value);
                  setActiveCommandIndex(0);
                }}
                onKeyDown={handlePaletteKeyDown}
              />
            </label>
            <ul id="command-palette-results" className="command-results" role="listbox">
              {filteredPaletteCommands.map((command, index) => (
                <li
                  id={`palette-option-${command.id}`}
                  key={command.id}
                  role="option"
                  aria-selected={index === selectedCommandIndex}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseMove={() => setActiveCommandIndex(index)}
                  onClick={() => activatePaletteCommand(command)}
                >
                  <span>{command.label}</span>
                  <small>{command.detail}</small>
                </li>
              ))}
              {filteredPaletteCommands.length === 0 && (
                <li className="command-empty" role="status">
                  No matching views, sources, or trends.
                </li>
              )}
            </ul>
            <p className="command-help">
              Use arrow keys to move, Enter to open, and Escape to close.
            </p>
          </div>
        </div>
      )}

      {undoId && (
        <div className="undo" role="status">
          <span>Feedback saved.</span>
          <button
            ref={undoButtonRef}
            onClick={() => {
              const id = undoId;
              void perform('Undoing feedback', () => transport.undoFeedback(id)).then((undone) => {
                if (undone) closeUndo();
              });
            }}
          >
            Undo
          </button>
          <button className="icon-button" aria-label="Dismiss feedback notice" onClick={closeUndo}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}

function PageHeader({
  eyebrow,
  title,
  detail,
  action,
}: {
  eyebrow: string;
  title: string;
  detail: string;
  action?: React.ReactNode;
}) {
  return (
    <header className="page-header">
      <div>
        <p className="eyebrow">{eyebrow}</p>
        <h1 tabIndex={-1}>{title}</h1>
        <p className="lede">{detail}</p>
      </div>
      {action}
    </header>
  );
}

function Today({
  dashboard,
  busy,
  onRefresh,
  onSync,
  onFeedback,
  onOpenOriginal,
  onAddSource,
  onImportArchive,
  onSave,
}: {
  dashboard: Dashboard;
  busy: boolean;
  onRefresh: () => void;
  onSync: () => void;
  onFeedback: (itemId: string, signal: FeedbackSignal) => void;
  onOpenOriginal: (url: string) => void;
  onAddSource: () => void;
  onImportArchive: () => void;
  onSave: (itemId: string, saved: boolean) => Promise<boolean>;
}) {
  if (dashboard.sources.length === 0) {
    return (
      <>
        <PageHeader
          eyebrow="Private by default"
          title="Choose your first source."
          detail="Web builds finite editions only from sources you deliberately add. Source data, summaries, and feedback stay in this app’s local data."
        />
        <section className="first-run" aria-labelledby="first-run-title">
          <div className="first-run-copy">
            <p className="eyebrow">Nothing is connected yet</p>
            <h2 id="first-run-title">Start with a source you trust.</h2>
            <p>
              Add a public RSS or Atom feed for bounded read-only updates, or select an official
              personal-data archive already on this computer. Web never posts, follows, likes, or
              asks for account passwords.
            </p>
            <div className="first-run-actions">
              <button className="primary" type="button" onClick={onAddSource}>
                Add an RSS feed
              </button>
              <button className="secondary" type="button" onClick={onImportArchive}>
                Import an official archive
              </button>
            </div>
          </div>
          <dl className="first-run-boundaries">
            <div>
              <dt>Local processing</dt>
              <dd>Summaries and ranking run locally, with a deterministic fallback.</dd>
            </div>
            <div>
              <dt>Read-only access</dt>
              <dd>Connected sources cannot change or publish to your accounts.</dd>
            </div>
            <div>
              <dt>Deliberate imports</dt>
              <dd>Only the official archive file you choose is read.</dd>
            </div>
          </dl>
        </section>
      </>
    );
  }

  return (
    <>
      <PageHeader
        eyebrow={dashboard.edition.label}
        title="Good morning."
        detail={dashboard.edition.summary}
        action={
          <div className="header-actions">
            <button className="primary" disabled={busy} onClick={onSync}>
              {busy ? 'Working…' : 'Sync all now (override retry timing)'}
            </button>
            <button className="secondary" disabled={busy} onClick={onRefresh}>
              Prepare from stored items
            </button>
          </div>
        }
      />
      <section className="edition-meta" aria-label="Edition timing">
        <div>
          <span>Last prepared</span>
          <strong>{formatDate(dashboard.edition.generatedAt)}</strong>
        </div>
        <div>
          <span>Automated schedule</span>
          <strong>{formatDate(dashboard.runner.nextScheduledAt)}</strong>
        </div>
        <div>
          <span>Edition size</span>
          <strong>{dashboard.items.length} useful items</strong>
        </div>
      </section>
      <section className="since-last-edition" aria-labelledby="since-last-edition-title">
        <div>
          <p className="eyebrow">A deliberate cadence</p>
          <h2 id="since-last-edition-title">Since your last edition</h2>
        </div>
        <p>{dashboard.sinceLastEdition.detail}</p>
      </section>
      <section aria-labelledby="attention-heading">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Finite by design</p>
            <h2 id="attention-heading">Worth your attention</h2>
          </div>
          <p>{dashboard.items.length} of today’s items</p>
        </div>
        <div className="card-grid">
          {dashboard.items.map((item, index) => (
            <DigestCard
              key={item.id}
              item={item}
              featured={index === 0}
              busy={busy}
              onFeedback={onFeedback}
              onOpenOriginal={onOpenOriginal}
              onSave={onSave}
            />
          ))}
        </div>
      </section>
      <div className="caught-up">
        <div aria-hidden="true">✓</div>
        <h2>You’re caught up.</h2>
        <p>
          There is nothing else to scroll through.{' '}
          {dashboard.runner.active
            ? 'The resident runner works only while Web is open; you can also synchronize deliberately.'
            : 'The runner is unavailable in this preview; prepare deliberately when you want a new edition.'}
        </p>
      </div>
    </>
  );
}

function DigestCard({
  item,
  featured,
  busy,
  onFeedback,
  onOpenOriginal,
  onSave,
}: {
  item: DigestItem;
  featured: boolean;
  busy: boolean;
  onFeedback: (itemId: string, signal: FeedbackSignal) => void;
  onOpenOriginal: (url: string) => void;
  onSave: (itemId: string, saved: boolean) => Promise<boolean>;
}) {
  const [expanded, setExpanded] = useState(false);
  const evidenceId = useId();
  return (
    <article className={featured ? 'digest-card featured' : 'digest-card'}>
      <div className="card-meta">
        <span className="topic">{item.topic}</span>
        <span>
          {timestampLabel(item.publishedTimeKind)} {formatDate(item.publishedAt)}
        </span>
      </div>
      <p className="source-line">
        {item.author} · {item.source}
      </p>
      <h3>{item.title}</h3>
      <p className="summary">{item.summary}</p>
      <p className="why">
        <span aria-hidden="true">◎</span> {item.reason}
      </p>
      <button
        className="text-button"
        aria-expanded={expanded}
        aria-controls={evidenceId}
        aria-label={`${expanded ? 'Hide' : 'Show'} evidence for ${item.title}`}
        onClick={() => setExpanded(!expanded)}
      >
        {expanded ? 'Hide evidence' : 'Show evidence & source details'}
      </button>
      {expanded && (
        <div className="evidence" id={evidenceId}>
          <p className="provenance-note">
            {item.summaryMethod === 'model'
              ? `Summary generated locally · ${item.summaryProvider}`
              : item.summaryMethod === 'extractive'
                ? 'Summary produced by the deterministic local fallback (no model installed)'
                : 'Demonstration fixture — not a real fetched source'}
            {item.summaryUncertainty ? ` · ${item.summaryUncertainty}` : ''}
          </p>
          {item.evidence.map((evidence, index) => (
            <blockquote key={`${evidence.source}-${evidence.publishedAt}-${index}`}>
              <p>“{evidence.excerpt}”</p>
              <footer>
                {evidence.author} · {evidence.source} · {timestampLabel(evidence.timestampKind)}{' '}
                {formatDate(evidence.publishedAt)}
                <br />
                {evidence.canonicalUrl ? (
                  <div className="source-url-actions">
                    <code>{evidence.canonicalUrl}</code>
                    {canOpenOriginal(evidence.canonicalUrl) ? (
                      <button
                        type="button"
                        className="text-button original-link-button"
                        disabled={busy}
                        aria-label={`Open original for ${item.title} from ${evidence.source}`}
                        onClick={() => {
                          const url = evidence.canonicalUrl;
                          if (canOpenOriginal(url)) onOpenOriginal(url);
                        }}
                      >
                        Open original
                      </button>
                    ) : (
                      <span className="source-url-note">
                        Copyable source URL; only credential-free HTTPS originals can be opened.
                      </span>
                    )}
                  </div>
                ) : (
                  <span>No canonical web URL was supplied.</span>
                )}
              </footer>
            </blockquote>
          ))}
        </div>
      )}
      <div className="feedback" aria-label={`Feedback for ${item.title}`}>
        <span>Was this useful?</span>
        <button data-save-item={item.id} disabled={busy} onClick={() => void onSave(item.id, true)}>
          Save for later
        </button>
        <button
          data-feedback-item={item.id}
          data-feedback-signal="more_like_this"
          disabled={busy}
          onClick={() => onFeedback(item.id, 'more_like_this')}
        >
          More like this
        </button>
        <button
          data-feedback-item={item.id}
          data-feedback-signal="less_like_this"
          disabled={busy}
          onClick={() => onFeedback(item.id, 'less_like_this')}
        >
          Less
        </button>
        <button
          data-feedback-item={item.id}
          data-feedback-signal="not_relevant"
          disabled={busy}
          onClick={() => onFeedback(item.id, 'not_relevant')}
        >
          Not relevant
        </button>
        <button
          data-feedback-item={item.id}
          data-feedback-signal="mute_source"
          disabled={busy}
          onClick={() => onFeedback(item.id, 'mute_source')}
        >
          Mute source
        </button>
        <small>
          More/Less bias future ranking for this source once it has enough signals; see Privacy
          &amp; settings for how. Not relevant removes this item now; Mute source removes this
          source’s items now.
        </small>
      </div>
    </article>
  );
}

function Trends({ dashboard }: { dashboard: Dashboard }) {
  return (
    <>
      <PageHeader
        eyebrow="Across independent sources"
        title="Trends, without the hype."
        detail="Deterministic lexical clustering groups posts that share enough overlapping significant terms, requires more than one distinct source, and collapses near-duplicate reposts to one representative before anything is shown. No model ever decides membership."
      />
      <div className="trend-list">
        {dashboard.trends.length === 0 && (
          <p className="empty-state">
            No cross-source trends meet the evidence threshold in this edition.
          </p>
        )}
        {dashboard.trends.map((trend) => (
          <article
            id={`trend-card-${trend.id}`}
            className="trend-card"
            key={trend.id}
            tabIndex={-1}
          >
            <div>
              <span className={`confidence ${trend.confidence}`}>{trend.confidence}</span>
              <span>
                {trend.method === 'fixture'
                  ? 'Demonstration fixture'
                  : `${trend.sourceCount} independent sources`}
              </span>
            </div>
            <h2>{trend.label}</h2>
            <p>{trend.summary}</p>
            <details>
              <summary>Evidence in this edition</summary>
              <ul>
                {trend.evidenceIds.map((id) => {
                  const item = dashboard.items.find((candidate) => candidate.id === id);
                  return <li key={id}>{item?.title ?? 'Source no longer available'}</li>;
                })}
              </ul>
            </details>
          </article>
        ))}
      </div>
      <p className="disclosure">
        Trends are produced by deterministic lexical clustering during digest preparation, gated by
        a cross-source requirement and a same-source dedup collapse. Membership is decided by that
        fixed logic alone; labels shown here are a deterministic fallback (the shared significant
        terms), not model-written. Muting or marking a member post not relevant hides its whole
        derived trend immediately.
      </p>
    </>
  );
}

function Library({
  dashboard,
  busy,
  onSave,
  onOpenOriginal,
  onViewSource,
  onExportSaved,
}: {
  dashboard: Dashboard;
  busy: boolean;
  onSave: (itemId: string, saved: boolean) => Promise<boolean>;
  onOpenOriginal: (url: string) => void;
  onViewSource: (sourceId: string) => void;
  onExportSaved: () => Promise<void>;
}) {
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<LibraryItem[]>([]);
  const [activeResultIndex, setActiveResultIndex] = useState(0);
  const [selectedItem, setSelectedItem] = useState<LibraryItem>();
  const [selectedEdition, setSelectedEdition] = useState<EditionDetail>();
  const [mode, setMode] = useState<'search' | 'saved'>('saved');
  const [status, setStatus] = useState(
    'Your explicitly saved items stay local beyond ordinary retention.',
  );
  const detailDialogRef = useRef<HTMLDivElement>(null);
  const detailCloseRef = useRef<HTMLButtonElement>(null);
  const detailReturnFocusRef = useRef<HTMLElement | null>(null);
  const runSearch = async (event: React.FormEvent) => {
    event.preventDefault();
    try {
      setMode('search');
      const result = await transport.searchLibrary(query);
      setItems(result);
      setActiveResultIndex(0);
      setStatus(result.length ? `${result.length} local matches.` : 'No retained local matches.');
    } catch (error) {
      setStatus(safeErrorMessage(error, 'Local search could not run.'));
    }
  };
  const loadSaved = async () => {
    try {
      setMode('saved');
      const result = await transport.savedLibrary();
      setItems(result);
      setActiveResultIndex(0);
      setStatus(result.length ? `${result.length} saved items.` : 'Nothing is saved yet.');
    } catch (error) {
      setStatus(safeErrorMessage(error, 'Saved items could not load.'));
    }
  };
  const updateSaved = async (itemId: string, saved: boolean) => {
    const updated = await onSave(itemId, saved);
    if (!updated) return;
    setItems((current) => {
      return saved || mode !== 'saved'
        ? current.map((item) => (item.id === itemId ? { ...item, saved } : item))
        : current.filter((item) => item.id !== itemId);
    });
    setActiveResultIndex(0);
    setSelectedItem((current) => (current?.id === itemId ? { ...current, saved } : current));
    setStatus(saved ? 'Saved for later.' : 'Removed from saved items.');
  };
  const openEdition = async (editionId: string) => {
    try {
      setSelectedEdition(await transport.getEdition(editionId));
      setStatus('Previous local edition opened.');
    } catch (error) {
      setStatus(safeErrorMessage(error, 'That edition is no longer available.'));
    }
  };
  useEffect(() => {
    let cancelled = false;
    void transport
      .savedLibrary()
      .then((result) => {
        if (cancelled) return;
        setMode('saved');
        setItems(result);
        setActiveResultIndex(0);
        setStatus(result.length ? `${result.length} saved items.` : 'Nothing is saved yet.');
      })
      .catch((error: unknown) => {
        if (!cancelled) setStatus(safeErrorMessage(error, 'Saved items could not load.'));
      });
    return () => {
      cancelled = true;
    };
  }, []);
  const focusLibraryResult = (index: number) => {
    if (!items.length) return;
    const nextIndex = Math.min(Math.max(index, 0), items.length - 1);
    setActiveResultIndex(nextIndex);
    window.requestAnimationFrame(() => {
      document.getElementById(`library-result-${nextIndex}`)?.focus();
    });
  };
  const openItemDetail = (item: LibraryItem) => {
    detailReturnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSelectedItem(item);
  };
  const closeItemDetail = () => {
    const returnTarget = detailReturnFocusRef.current;
    detailReturnFocusRef.current = null;
    setSelectedItem(undefined);
    window.requestAnimationFrame(() => returnTarget?.focus());
  };
  const trapItemDetailFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      closeItemDetail();
      return;
    }
    if (event.key !== 'Tab') return;
    const focusable = detailDialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), [href], input:not([disabled])',
    );
    if (!focusable?.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  useEffect(() => {
    if (!selectedItem) return;
    const frame = window.requestAnimationFrame(() => detailCloseRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [selectedItem]);
  return (
    <>
      <PageHeader
        eyebrow="Private recall"
        title="Library."
        detail="Search only the material you deliberately retain, or keep a small explicit read-later shelf."
      />
      <form className="add-source library-search" onSubmit={(event) => void runSearch(event)}>
        <label htmlFor="library-query">Search your local library</label>
        <div className="library-search-row">
          <input
            id="library-query"
            value={query}
            maxLength={200}
            onChange={(event) => setQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'ArrowDown' && items.length) {
                event.preventDefault();
                focusLibraryResult(0);
              }
            }}
            placeholder="People, ideas, places, or words you remember"
          />
          <button className="primary" disabled={busy || !query.trim()}>
            Search
          </button>
          <button
            type="button"
            className="secondary"
            disabled={busy}
            onClick={() => void loadSaved()}
          >
            Saved ({dashboard.library.savedCount})
          </button>
          <button
            type="button"
            className="secondary"
            disabled={busy || dashboard.library.savedCount === 0}
            onClick={() => void onExportSaved()}
          >
            Export saved items
          </button>
        </div>
        <p className="field-message" role="status">
          {status}
        </p>
        {items.length > 0 && (
          <p id="library-keyboard-help" className="field-message compact">
            From the search field, press Down Arrow to enter results. Use Up/Down, Home, or End to
            move between result cards.
          </p>
        )}
      </form>
      <section
        className="library-list"
        aria-label={mode === 'saved' ? 'Saved items' : 'Search results'}
        aria-describedby={items.length > 0 ? 'library-keyboard-help' : undefined}
      >
        {items.map((item, index) => (
          <article
            id={`library-result-${index}`}
            className="library-card"
            key={item.id}
            tabIndex={index === activeResultIndex ? 0 : -1}
            aria-label={`${mode === 'saved' ? 'Saved item' : 'Search result'} ${index + 1} of ${items.length}: ${item.title}`}
            onKeyDown={(event) => {
              if (event.target !== event.currentTarget) return;
              if (event.key === 'ArrowDown') {
                event.preventDefault();
                focusLibraryResult(index + 1);
              } else if (event.key === 'ArrowUp') {
                event.preventDefault();
                focusLibraryResult(index - 1);
              } else if (event.key === 'Home') {
                event.preventDefault();
                focusLibraryResult(0);
              } else if (event.key === 'End') {
                event.preventDefault();
                focusLibraryResult(items.length - 1);
              }
            }}
          >
            <p className="source-line">
              {item.author} · {item.source} · {formatDate(item.publishedAt)}
            </p>
            <h2>{item.title}</h2>
            <p>{item.excerpt}</p>
            <div className="library-actions">
              <button
                className="text-button"
                aria-label={`Read details for ${item.title}`}
                onClick={() => openItemDetail(item)}
              >
                Read details
              </button>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => void updateSaved(item.id, !item.saved)}
              >
                {item.saved ? 'Remove from saved' : 'Save for later'}
              </button>
              {canOpenOriginal(item.canonicalUrl) && (
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() => {
                    const url = item.canonicalUrl;
                    if (canOpenOriginal(url)) onOpenOriginal(url);
                  }}
                >
                  Open original
                </button>
              )}
            </div>
          </article>
        ))}
        {items.length === 0 && (
          <p className="empty-state">
            {mode === 'saved'
              ? 'Save a useful item from an edition to return to it later.'
              : 'Search remains local and is limited to retained sources.'}
          </p>
        )}
      </section>
      {selectedItem && (
        <div
          className="command-backdrop item-detail-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget) closeItemDetail();
          }}
        >
          <div
            ref={detailDialogRef}
            className="command-dialog item-detail-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="library-item-detail-title"
            onKeyDown={trapItemDetailFocus}
          >
            <div className="command-dialog-header">
              <div>
                <p className="eyebrow">Retained local item</p>
                <h2 id="library-item-detail-title">{selectedItem.title}</h2>
              </div>
              <button
                ref={detailCloseRef}
                className="secondary command-close"
                type="button"
                onClick={closeItemDetail}
              >
                Close details
              </button>
            </div>
            <div className="item-detail-body">
              <p className="source-line">
                {selectedItem.author} · {selectedItem.source} ·{' '}
                {formatDate(selectedItem.publishedAt)}
              </p>
              <p>{selectedItem.excerpt}</p>
              <p className="hint">
                <strong>Source health:</strong> {statusLabel(selectedItem.sourceStatus)}
                {selectedItem.sourceHealthDetail ? ` — ${selectedItem.sourceHealthDetail}` : ''}
              </p>
              <p className="hint">
                <strong>Summary:</strong>{' '}
                {selectedItem.summaryMethod
                  ? `${statusLabel(selectedItem.summaryMethod)} via ${selectedItem.summaryProvider ?? 'local processing'}${selectedItem.summaryUncertainty ? ` — ${selectedItem.summaryUncertainty}` : ''}`
                  : 'No generated summary is retained for this item.'}
              </p>
              <p className="hint">
                This is the retained local excerpt. Open the attributed original only when you
                choose.
              </p>
              <div className="library-actions">
                <button className="secondary" onClick={() => onViewSource(selectedItem.sourceId)}>
                  View source
                </button>
                <button
                  className="secondary"
                  disabled={busy}
                  onClick={() => void updateSaved(selectedItem.id, !selectedItem.saved)}
                >
                  {selectedItem.saved ? 'Remove from saved' : 'Save for later'}
                </button>
                {canOpenOriginal(selectedItem.canonicalUrl) && (
                  <button
                    className="secondary"
                    disabled={busy}
                    onClick={() => {
                      const url = selectedItem.canonicalUrl;
                      if (canOpenOriginal(url)) onOpenOriginal(url);
                    }}
                  >
                    Open original
                  </button>
                )}
              </div>
            </div>
          </div>
        </div>
      )}
      <section className="edition-history" aria-labelledby="edition-history-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Finite history</p>
            <h2 id="edition-history-title">Previous editions</h2>
          </div>
          <p>Latest {dashboard.history.length}</p>
        </div>
        {dashboard.history.map((edition) => (
          <article className="history-row" key={edition.id}>
            <div>
              <strong>{edition.label}</strong>
              <p>{edition.summary}</p>
            </div>
            <div className="history-actions">
              <span>
                {edition.itemCount} items · {formatDate(edition.generatedAt)}
              </span>
              <button
                className="text-button"
                disabled={busy}
                onClick={() => void openEdition(edition.id)}
              >
                Read edition
              </button>
            </div>
          </article>
        ))}
      </section>
      {selectedEdition && (
        <section className="past-edition" aria-labelledby="past-edition-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">From your local history</p>
              <h2 id="past-edition-title">{selectedEdition.edition.label}</h2>
            </div>
            <button className="text-button" onClick={() => setSelectedEdition(undefined)}>
              Close
            </button>
          </div>
          <p className="hint">
            {formatDate(selectedEdition.edition.generatedAt)} · {selectedEdition.edition.summary}
          </p>
          <div className="library-list">
            {selectedEdition.items.map((item) => (
              <article className="library-card" key={item.id}>
                <p className="source-line">
                  {item.author} · {item.source}
                </p>
                <h3>{item.title}</h3>
                <p>{item.summary}</p>
              </article>
            ))}
          </div>
        </section>
      )}
    </>
  );
}

function Sources({
  dashboard,
  busy,
  onAdd,
  onImport,
  onDelete,
  onRename,
  onSetPaused,
  onPickOpml,
  onDiscover,
  onProbeMastodon,
  onConnectMastodon,
  onSyncSource,
  statusMessage,
  statusIsError,
}: {
  dashboard: Dashboard;
  busy: boolean;
  onAdd: (label: string, url: string) => Promise<boolean>;
  onImport: (platform: ArchiveImportPlatform, label: string) => Promise<boolean>;
  onDelete: (source: Source) => Promise<boolean>;
  onRename: (sourceId: string, label: string) => Promise<boolean>;
  onSetPaused: (sourceId: string, paused: boolean) => Promise<boolean>;
  onPickOpml: () => Promise<OpmlCandidate[]>;
  onDiscover: (url: string) => Promise<OpmlCandidate[]>;
  onProbeMastodon: (instanceUrl: string) => Promise<MastodonProbeResult>;
  onConnectMastodon: (label: string, instanceUrl: string) => Promise<boolean>;
  onSyncSource: (sourceId: string) => Promise<boolean>;
  statusMessage: string;
  statusIsError: boolean;
}) {
  const [label, setLabel] = useState('');
  const [url, setUrl] = useState('');
  const [importPlatform, setImportPlatform] = useState<ArchiveImportPlatform>('x');
  const [importLabel, setImportLabel] = useState('My X archive');
  const [opmlCandidates, setOpmlCandidates] = useState<FeedReviewCandidate[]>([]);
  const [feedImportResults, setFeedImportResults] = useState<FeedImportResult[]>([]);
  const [opmlNotice, setOpmlNotice] = useState('');
  const [mastodonInstanceUrl, setMastodonInstanceUrl] = useState('');
  const [mastodonProbe, setMastodonProbe] = useState<MastodonProbeResult>();
  const [mastodonNotice, setMastodonNotice] = useState('');
  const [mastodonLabel, setMastodonLabel] = useState('My Mastodon');
  const [sourceDialog, setSourceDialog] = useState<SourceDialog>();
  const [sourceLabelDraft, setSourceLabelDraft] = useState('');
  const sourceDialogRef = useRef<HTMLDivElement>(null);
  const sourceDialogInputRef = useRef<HTMLInputElement>(null);
  const sourceDialogReturnFocusRef = useRef<HTMLElement | null>(null);
  const openSourceDialog = (action: SourceDialog['action'], source: Source) => {
    sourceDialogReturnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSourceLabelDraft(source.label);
    setSourceDialog({ action, source });
  };
  const closeSourceDialog = (restoreFocus = true) => {
    setSourceDialog(undefined);
    const returnTarget = sourceDialogReturnFocusRef.current;
    sourceDialogReturnFocusRef.current = null;
    if (restoreFocus) window.requestAnimationFrame(() => returnTarget?.focus());
  };
  const trapSourceDialogFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab') return;
    const focusable = sourceDialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), [href]',
    );
    if (!focusable?.length) return;
    const first = focusable[0]!;
    const last = focusable[focusable.length - 1]!;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  useEffect(() => {
    if (!sourceDialog) return;
    const frame = window.requestAnimationFrame(() => {
      if (sourceDialog.action === 'rename') sourceDialogInputRef.current?.focus();
      else sourceDialogRef.current?.querySelector<HTMLElement>('button:not([disabled])')?.focus();
    });
    return () => window.cancelAnimationFrame(frame);
  }, [sourceDialog]);
  useEffect(() => {
    if (!sourceDialog) return;
    const dismissWithEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) {
        event.preventDefault();
        closeSourceDialog();
      }
    };
    document.addEventListener('keydown', dismissWithEscape);
    return () => document.removeEventListener('keydown', dismissWithEscape);
  }, [busy, sourceDialog]);
  const updateReviewedCandidate = (
    reviewId: string,
    field: keyof Pick<FeedReviewCandidate, 'label' | 'url'>,
    value: string,
  ) => {
    setOpmlCandidates((current) =>
      current.map((candidate) =>
        candidate.reviewId === reviewId ? { ...candidate, [field]: value } : candidate,
      ),
    );
  };
  const retryReviewedCandidate = async (candidate: FeedReviewCandidate) => {
    const added = await onAdd(candidate.label, candidate.url);
    setFeedImportResults((current) =>
      current.map((result) =>
        result.reviewId === candidate.reviewId ? { ...candidate, added } : result,
      ),
    );
    if (added) {
      setOpmlCandidates((current) => current.filter((row) => row.reviewId !== candidate.reviewId));
      setOpmlNotice(`Added ${candidate.label}.`);
    } else {
      setOpmlNotice(
        `Could not add ${candidate.label}. Edit it or try again when the feed is ready.`,
      );
    }
  };
  return (
    <>
      <PageHeader
        eyebrow="Read-only connections"
        title="Your sources."
        detail="Credentials are never sent to the interface. Live social connections require official OAuth support and minimum read scopes."
      />
      <form
        className="add-source"
        aria-describedby="rss-operation-status"
        onSubmit={(event) => {
          event.preventDefault();
          void onAdd(label, url).then((added) => {
            if (added) {
              setLabel('');
              setUrl('');
            }
          });
        }}
      >
        <div>
          <label htmlFor="rss-label">Feed name</label>
          <input
            id="rss-label"
            required
            maxLength={100}
            value={label}
            onChange={(event) => setLabel(event.target.value)}
            placeholder="A useful publication"
          />
        </div>
        <div>
          <label htmlFor="rss-url">RSS or Atom URL</label>
          <input
            id="rss-url"
            required
            type="url"
            value={url}
            aria-invalid={statusIsError}
            aria-describedby={statusIsError ? 'rss-url-help rss-operation-status' : 'rss-url-help'}
            onChange={(event) => setUrl(event.target.value)}
            placeholder="https://example.com/feed.xml"
          />
          <span id="rss-url-help" className="field-message compact">
            Use a public http or https RSS/Atom address.
          </span>
        </div>
        <button className="primary" disabled={busy}>
          Add read-only feed
        </button>
        <button
          type="button"
          className="secondary"
          disabled={busy || !url}
          onClick={() =>
            void onDiscover(url)
              .then((candidates) => {
                setOpmlCandidates(reviewCandidates(candidates));
                setFeedImportResults([]);
                setOpmlNotice(
                  candidates.length
                    ? `${candidates.length} advertised feeds are ready for review.`
                    : 'No advertised feeds were found.',
                );
              })
              .catch((error) =>
                setOpmlNotice(safeErrorMessage(error, 'That website could not be safely checked.')),
              )
          }
        >
          Find feeds on this website
        </button>
        <p>
          Paste either a feed address or a publication website. Web checks at most 512 KB for
          advertised feeds (and at most 2 MB per connected feed), blocks private-network targets,
          follows no more than three validated redirects, and stores excerpts with attribution.
        </p>
        <p
          id="rss-operation-status"
          className={statusIsError ? 'field-message error' : 'field-message'}
        >
          {statusMessage}
        </p>
      </form>
      <section className="archive-import-section" aria-labelledby="archive-import-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Private history</p>
            <h2 id="archive-import-title">Import an official data archive</h2>
          </div>
          <p>Local file · no upload</p>
        </div>
        <p className="hint">
          Bring your own posts from an official X or Instagram export into Web. Rust reads only the
          file you choose, accepts at most 20 MiB and 25,000 entries per import, and never uploads
          it. Re-import the same named archive later to add or update posts.
        </p>
        <form
          className="add-source archive-import"
          onSubmit={(event) => {
            event.preventDefault();
            void onImport(importPlatform, importLabel);
          }}
        >
          <div>
            <label htmlFor="archive-platform">Archive platform</label>
            <select
              id="archive-platform"
              value={importPlatform}
              onChange={(event) => {
                const platform = event.target.value as ArchiveImportPlatform;
                setImportPlatform(platform);
                setImportLabel(platform === 'x' ? 'My X archive' : 'My Instagram archive');
              }}
            >
              <option value="x">X (Twitter)</option>
              <option value="instagram">Instagram</option>
            </select>
          </div>
          <div>
            <label htmlFor="archive-label">Archive name</label>
            <input
              id="archive-label"
              required
              maxLength={100}
              value={importLabel}
              onChange={(event) => setImportLabel(event.target.value)}
            />
            <span className="field-message compact">
              Use the same name for later imports from this archive.
            </span>
          </div>
          <button className="primary" disabled={busy}>
            Choose archive file
          </button>
          <p className="archive-file-help">
            {importPlatform === 'x'
              ? 'Choose data/tweets.js (or tweet.js) from the extracted X archive.'
              : 'Choose posts_1.json from your extracted Instagram activity archive.'}
          </p>
        </form>
      </section>
      <section className="opml-import-section" aria-labelledby="opml-import-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Move in gently</p>
            <h2 id="opml-import-title">Bring an OPML feed list</h2>
          </div>
          <p>Review before adding</p>
        </div>
        <p className="hint">
          Choose a local OPML file. Web reads at most 1 MiB and shows public RSS/Atom URLs before it
          connects anything.
        </p>
        <button
          className="secondary"
          disabled={busy}
          onClick={() =>
            void onPickOpml()
              .then((candidates) => {
                setOpmlCandidates(reviewCandidates(candidates));
                setFeedImportResults([]);
                setOpmlNotice(
                  candidates.length
                    ? `${candidates.length} feeds are ready for review.`
                    : 'No OPML file was selected.',
                );
              })
              .catch((error) =>
                setOpmlNotice(safeErrorMessage(error, 'The OPML list could not be read.')),
              )
          }
        >
          Choose OPML file
        </button>
        {opmlNotice && (
          <p className="field-message" role="status">
            {opmlNotice}
          </p>
        )}
        {opmlCandidates.length > 0 && (
          <div className="opml-review">
            <p>
              <strong>
                {feedImportResults.some((result) => !result.added)
                  ? 'Only feeds that did not connect remain below. Edit or retry them individually.'
                  : 'Nothing is connected until you choose Add reviewed feeds.'}
              </strong>
            </p>
            {feedImportResults.some((result) => !result.added) ? (
              <div className="failed-feed-review">
                {opmlCandidates.map((candidate) => (
                  <fieldset className="failed-feed-row" key={candidate.reviewId}>
                    <legend>Feed that needs attention</legend>
                    <label htmlFor={`review-label-${candidate.reviewId}`}>Feed name</label>
                    <input
                      id={`review-label-${candidate.reviewId}`}
                      maxLength={100}
                      value={candidate.label}
                      onChange={(event) =>
                        updateReviewedCandidate(candidate.reviewId, 'label', event.target.value)
                      }
                    />
                    <label htmlFor={`review-url-${candidate.reviewId}`}>RSS or Atom URL</label>
                    <input
                      id={`review-url-${candidate.reviewId}`}
                      type="url"
                      value={candidate.url}
                      onChange={(event) =>
                        updateReviewedCandidate(candidate.reviewId, 'url', event.target.value)
                      }
                    />
                    <div className="source-actions">
                      <button
                        className="secondary"
                        disabled={busy || !candidate.label.trim() || !candidate.url.trim()}
                        onClick={() => void retryReviewedCandidate(candidate)}
                      >
                        Retry {candidate.label}
                      </button>
                      <button
                        className="text-button"
                        disabled={busy}
                        onClick={() => {
                          setOpmlCandidates((current) =>
                            current.filter((row) => row.reviewId !== candidate.reviewId),
                          );
                          setFeedImportResults((current) =>
                            current.filter((result) => result.reviewId !== candidate.reviewId),
                          );
                        }}
                      >
                        Remove from review
                      </button>
                    </div>
                  </fieldset>
                ))}
              </div>
            ) : (
              <>
                <ul>
                  {opmlCandidates.slice(0, 12).map((candidate) => (
                    <li key={candidate.reviewId}>
                      {candidate.label} <small>{candidate.url}</small>
                    </li>
                  ))}
                  {opmlCandidates.length > 12 && <li>…and {opmlCandidates.length - 12} more.</li>}
                </ul>
                <button
                  className="primary"
                  disabled={busy}
                  onClick={() =>
                    void (async () => {
                      const results: FeedImportResult[] = [];
                      for (const candidate of opmlCandidates) {
                        results.push({
                          ...candidate,
                          added: await onAdd(candidate.label, candidate.url),
                        });
                      }
                      setFeedImportResults(results);
                      const added = results.filter((result) => result.added).length;
                      const failed = results.filter((result) => !result.added);
                      setOpmlCandidates(failed);
                      setOpmlNotice(
                        failed.length
                          ? `Added ${added} of ${results.length} reviewed feeds. Edit or retry only the ${failed.length} feed${failed.length === 1 ? '' : 's'} that did not connect.`
                          : `Added ${added} reviewed feeds.`,
                      );
                    })()
                  }
                >
                  Add reviewed feeds
                </button>
              </>
            )}
            <button className="text-button" disabled={busy} onClick={() => setOpmlCandidates([])}>
              Discard review
            </button>
          </div>
        )}
        {feedImportResults.length > 0 && (
          <ul className="feed-import-results" aria-label="Feed import results">
            {feedImportResults.map((result) => (
              <li key={result.reviewId} className={result.added ? 'success' : 'failure'}>
                {result.label}: {result.added ? 'added' : 'not added'}
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-labelledby="official-connectors-title">
        <h2 id="official-connectors-title">Official social connectors</h2>
        <p className="hint">
          These read-only connectors stay unavailable until their external OAuth and policy gates
          have evidence. Web does not accept credentials for them in this build.
        </p>
        <div className="source-list">
          {dashboard.connectors
            .filter((connector) => connector.kind !== 'rss')
            .map((connector) => (
              <article className="source-card" key={connector.kind}>
                <div className="source-icon" aria-hidden="true">
                  {connector.label.slice(0, 1)}
                </div>
                <div>
                  <div className="source-title">
                    <h3>{connector.label}</h3>
                    <span className="source-status paused">
                      <span className="status-dot paused" />
                      Not available
                    </span>
                  </div>
                  <p>{connector.detail}</p>
                  {connector.unmetPrerequisite && (
                    <p className="field-message">Required first: {connector.unmetPrerequisite}</p>
                  )}
                  {connector.kind === 'mastodon' && (
                    <form
                      className="mastodon-compatibility"
                      aria-describedby="mastodon-compatibility-help"
                      onSubmit={(event) => {
                        event.preventDefault();
                        void onProbeMastodon(mastodonInstanceUrl)
                          .then((result) => {
                            setMastodonProbe(result);
                            setMastodonNotice(
                              `Compatible read scopes: ${result.supportedScopes.join(', ')}. You can now authorize read-only access.`,
                            );
                          })
                          .catch((error) =>
                            setMastodonNotice(
                              safeErrorMessage(
                                error,
                                'This instance could not be checked. No account was connected.',
                              ),
                            ),
                          );
                      }}
                    >
                      <label htmlFor="mastodon-instance-url">Mastodon instance</label>
                      <input
                        id="mastodon-instance-url"
                        required
                        type="url"
                        inputMode="url"
                        autoComplete="url"
                        maxLength={2048}
                        value={mastodonInstanceUrl}
                        onChange={(event) => {
                          setMastodonInstanceUrl(event.target.value);
                          setMastodonProbe(undefined);
                        }}
                        placeholder="https://social.example/"
                      />
                      <button className="secondary" disabled={busy || !isTauri()}>
                        Check compatibility
                      </button>
                      {mastodonProbe && (
                        <>
                          <label htmlFor="mastodon-label">Source name</label>
                          <input
                            id="mastodon-label"
                            maxLength={100}
                            value={mastodonLabel}
                            onChange={(event) => setMastodonLabel(event.target.value)}
                          />
                          <button
                            type="button"
                            className="primary"
                            disabled={busy || !mastodonLabel.trim() || !isTauri()}
                            onClick={() =>
                              void onConnectMastodon(mastodonLabel, mastodonInstanceUrl).then(
                                (connected) => {
                                  if (connected)
                                    setMastodonNotice(
                                      'Authorization completed. Your read-only source is ready to synchronize.',
                                    );
                                },
                              )
                            }
                          >
                            Authorize read-only Mastodon
                          </button>
                        </>
                      )}
                      <p id="mastodon-compatibility-help" className="hint">
                        Check compatibility first. Authorize opens your system browser only after a
                        compatible result; Web requests read scopes only and stores its token in the
                        OS vault.
                      </p>
                      {!isTauri() && (
                        <p className="field-message">
                          Compatibility checks need the native desktop app; browser preview makes no
                          provider request.
                        </p>
                      )}
                      {mastodonProbe && (
                        <p className="field-message">
                          {mastodonProbe.instanceUrl} is compatible. It is not connected.
                        </p>
                      )}
                      {mastodonNotice && (
                        <p
                          id="mastodon-compatibility-status"
                          className="field-message"
                          role="status"
                        >
                          {mastodonNotice}
                        </p>
                      )}
                    </form>
                  )}
                  <p className="hint">
                    Read-only; no posting, likes, direct messages, search, or contact discovery.
                  </p>
                </div>
              </article>
            ))}
        </div>
      </section>
      <div className="source-list">
        {dashboard.sources.length === 0 && (
          <p className="empty-state">No sources are connected yet. Add a read-only feed above.</p>
        )}
        {dashboard.sources.map((source) => (
          <article
            id={`source-card-${source.id}`}
            className="source-card"
            key={source.id}
            tabIndex={-1}
          >
            <div className="source-icon" aria-hidden="true">
              {source.kind.slice(0, 1).toUpperCase()}
            </div>
            <div>
              <div className="source-title">
                <h2>{source.label}</h2>
                <span className={`source-status ${source.status}`}>
                  <span className={`status-dot ${source.status}`} />
                  {statusLabel(source.status)}
                </span>
              </div>
              <p>{source.detail}</p>
              {source.healthDetail && <p className="hint">{source.healthDetail}</p>}
              <dl>
                <div>
                  <dt>{source.kind === 'archive_import' ? 'Imported' : 'Last sync'}</dt>
                  <dd>{formatDate(source.lastSync)}</dd>
                </div>
                <div>
                  <dt>
                    {source.kind === 'archive_import' ? 'Refresh' : 'Next eligible poll / retry'}
                  </dt>
                  <dd>
                    {source.kind === 'archive_import'
                      ? 'Manual re-import only'
                      : source.nextSync === null
                        ? 'Eligible now'
                        : formatDate(source.nextSync)}
                  </dd>
                </div>
                <div>
                  <dt>Stored items</dt>
                  <dd>{source.itemCount}</dd>
                </div>
                <div>
                  <dt>Comments</dt>
                  <dd>
                    {source.commentsStatus === 'unavailable'
                      ? 'Unavailable from this source'
                      : `${source.commentsStatus}${source.commentsTruncated ? ' · truncated' : ''}`}
                  </dd>
                </div>
                <div>
                  <dt>{source.kind === 'archive_import' ? 'Import result' : 'Last page'}</dt>
                  <dd>
                    {source.syncFinality === 'partial' ? 'Partial · more may remain' : 'Complete'}
                  </dd>
                </div>
              </dl>
            </div>
            <div className="source-actions">
              <button
                className="text-button"
                disabled={busy}
                onClick={() => openSourceDialog('rename', source)}
              >
                Rename
              </button>
              {source.kind === 'rss' && (
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() => void onSetPaused(source.id, source.status !== 'paused')}
                >
                  {source.status === 'paused' ? 'Resume' : 'Pause'}
                </button>
              )}
              {source.kind === 'rss' && source.status !== 'paused' && (
                <button
                  className="text-button"
                  disabled={busy}
                  onClick={() => void onSyncSource(source.id)}
                >
                  Sync now
                </button>
              )}
            </div>
            <button
              className="danger-text"
              disabled={busy}
              onClick={() => openSourceDialog('delete', source)}
            >
              {source.kind === 'archive_import'
                ? 'Delete import & local data'
                : 'Disconnect & delete'}
            </button>
          </article>
        ))}
      </div>
      <p className="hint">
        The resident runner selects due sources only. “Sync all now” is a deliberate override of a
        source’s next eligible retry, still subject to the per-run source and time bounds.
      </p>
      <div className="boundary-note">
        <strong>Why aren’t all networks here?</strong>
        <p>
          Many platforms do not provide an official personal home-feed API. Web does not disguise
          automation, bypass access controls, import session cookies, or claim unsupported coverage.
        </p>
      </div>
      {sourceDialog && (
        <div
          className="command-backdrop source-dialog-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget && !busy) closeSourceDialog();
          }}
        >
          <div
            ref={sourceDialogRef}
            className="command-dialog source-action-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="source-action-title"
            aria-describedby="source-action-description"
            onKeyDown={trapSourceDialogFocus}
          >
            <div className="command-dialog-header">
              <div>
                <p className="eyebrow">
                  {sourceDialog.action === 'rename' ? 'Source details' : 'Local data'}
                </p>
                <h2 id="source-action-title">
                  {sourceDialog.action === 'rename'
                    ? `Rename ${sourceDialog.source.label}`
                    : `Delete ${sourceDialog.source.label}?`}
                </h2>
              </div>
              <button
                className="secondary command-close"
                type="button"
                disabled={busy}
                onClick={() => closeSourceDialog()}
              >
                Close
              </button>
            </div>
            {sourceDialog.action === 'rename' ? (
              <form
                className="source-action-body"
                onSubmit={(event) => {
                  event.preventDefault();
                  const nextLabel = sourceLabelDraft.trim();
                  if (!nextLabel || nextLabel === sourceDialog.source.label) {
                    closeSourceDialog();
                    return;
                  }
                  void onRename(sourceDialog.source.id, nextLabel).then((renamed) => {
                    if (renamed) closeSourceDialog();
                  });
                }}
              >
                <p id="source-action-description">
                  This changes the private label shown on this device. The feed address stays the
                  same.
                </p>
                <label htmlFor="source-label-draft">Source name</label>
                <input
                  ref={sourceDialogInputRef}
                  id="source-label-draft"
                  required
                  maxLength={100}
                  value={sourceLabelDraft}
                  onChange={(event) => setSourceLabelDraft(event.target.value)}
                />
                <div className="source-dialog-actions">
                  <button
                    type="button"
                    className="secondary"
                    disabled={busy}
                    onClick={() => closeSourceDialog()}
                  >
                    Cancel
                  </button>
                  <button className="primary" disabled={busy || !sourceLabelDraft.trim()}>
                    Save name
                  </button>
                </div>
              </form>
            ) : (
              <div className="source-action-body">
                <p id="source-action-description">
                  {sourceDialog.source.kind === 'archive_import'
                    ? 'This permanently removes this import and its locally stored posts, summaries, and feedback from this device.'
                    : 'This disconnects the source and permanently removes its local posts, summaries, feedback, and credentials from this device.'}
                </p>
                <p className="source-delete-warning">
                  This cannot be undone. Export a local backup first if you may want this data
                  later.
                </p>
                <div className="source-dialog-actions">
                  <button
                    type="button"
                    className="secondary"
                    disabled={busy}
                    onClick={() => closeSourceDialog()}
                  >
                    Keep source
                  </button>
                  <button
                    type="button"
                    className="danger-button"
                    disabled={busy}
                    onClick={() =>
                      void onDelete(sourceDialog.source).then((deleted) => {
                        if (deleted) closeSourceDialog(false);
                      })
                    }
                  >
                    {sourceDialog.source.kind === 'archive_import'
                      ? 'Delete import and local data'
                      : 'Disconnect and delete'}
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      )}
    </>
  );
}

function Activity({ dashboard }: { dashboard: Dashboard }) {
  const healthySources = dashboard.sources.filter((source) => source.status === 'healthy').length;
  const pausedSources = dashboard.sources.filter((source) => source.status === 'paused').length;
  const attentionSources = dashboard.sources.length - healthySources - pausedSources;
  const runnerState = dashboard.runner.inFlight
    ? 'Running now'
    : dashboard.runner.active
      ? `Ready · ${statusLabel(dashboard.runner.lastOutcome)}`
      : 'Inactive';
  const modelState =
    dashboard.model.state === 'ready'
      ? dashboard.model.model
      : `Fallback · ${statusLabel(dashboard.model.state)}`;

  return (
    <>
      <PageHeader
        eyebrow="Quietly accountable"
        title="Activity."
        detail="A concise local history of sync, model, and schedule work. Post text, prompts, credentials, and private URLs are never logged here."
      />
      <section className="activity-overview" aria-labelledby="activity-overview-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Current local state</p>
            <h2 id="activity-overview-title">System snapshot</h2>
          </div>
          <p>Visible only on this computer</p>
        </div>
        <dl className="activity-vitals">
          <div>
            <dt>Connected sources</dt>
            <dd>{dashboard.sources.length}</dd>
            <small>{healthySources} healthy</small>
          </div>
          <div>
            <dt>Need attention</dt>
            <dd>{attentionSources}</dd>
            <small>{pausedSources} paused</small>
          </div>
          <div>
            <dt>Resident runner</dt>
            <dd>{runnerState}</dd>
            <small>{formatDate(dashboard.runner.nextScheduledAt)}</small>
          </div>
          <div>
            <dt>Local summary path</dt>
            <dd>{modelState}</dd>
            <small>{dashboard.model.provider}</small>
          </div>
        </dl>
      </section>

      <section className="activity-system-grid" aria-label="Runner and model status">
        <article className="activity-panel">
          <div className="activity-panel-heading">
            <div>
              <p className="eyebrow">Resident work</p>
              <h2>Runner</h2>
            </div>
            <span className={`activity-state ${dashboard.runner.lastOutcome}`}>{runnerState}</span>
          </div>
          <dl className="activity-detail-list">
            <div>
              <dt>Last attempt</dt>
              <dd>{formatDate(dashboard.runner.lastAttemptAt)}</dd>
            </div>
            <div>
              <dt>Last success</dt>
              <dd>{formatDate(dashboard.runner.lastSuccessAt)}</dd>
            </div>
            <div>
              <dt>Next eligible run</dt>
              <dd>{formatDate(dashboard.runner.nextScheduledAt)}</dd>
            </div>
          </dl>
          <p className="hint">{dashboard.runner.detail}</p>
        </article>

        <article className="activity-panel">
          <div className="activity-panel-heading">
            <div>
              <p className="eyebrow">Local inference</p>
              <h2>Model path</h2>
            </div>
            <span className={`model-state ${dashboard.model.state}`}>
              {statusLabel(dashboard.model.state)}
            </span>
          </div>
          <dl className="activity-detail-list">
            <div>
              <dt>Provider</dt>
              <dd>{dashboard.model.provider}</dd>
            </div>
            <div>
              <dt>Selected model</dt>
              <dd>{dashboard.model.model ?? 'Deterministic fallback'}</dd>
            </div>
            <div>
              <dt>Fallback</dt>
              <dd>{dashboard.model.fallbackAvailable ? 'Available' : 'Unavailable'}</dd>
            </div>
          </dl>
          <p className="hint">{dashboard.model.detail}</p>
        </article>
      </section>

      <section className="source-health" aria-labelledby="source-health-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Read-only connections</p>
            <h2 id="source-health-title">Source health</h2>
          </div>
          <p>{dashboard.sources.length} connected</p>
        </div>
        <div className="table-scroll">
          <table className="source-health-table">
            <caption className="sr-only">
              Current health, item count, and synchronization timing for connected sources
            </caption>
            <thead>
              <tr>
                <th scope="col">Source</th>
                <th scope="col">Status</th>
                <th scope="col">Items</th>
                <th scope="col">Last sync</th>
                <th scope="col">Next eligible</th>
              </tr>
            </thead>
            <tbody>
              {dashboard.sources.map((source) => (
                <tr key={source.id}>
                  <th scope="row">{source.label}</th>
                  <td>
                    <span className={`source-status ${source.status}`}>
                      <span className={`status-dot ${source.status}`} aria-hidden="true" />
                      {statusLabel(source.status)}
                    </span>
                  </td>
                  <td>{source.itemCount}</td>
                  <td>{formatDate(source.lastSync)}</td>
                  <td>
                    {source.kind === 'archive_import'
                      ? 'Manual re-import only'
                      : source.nextSync === null
                        ? 'Eligible now'
                        : formatDate(source.nextSync)}
                  </td>
                </tr>
              ))}
              {dashboard.sources.length === 0 && (
                <tr>
                  <td colSpan={5}>No connected sources to report.</td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </section>

      <section aria-labelledby="activity-history-title">
        <div className="section-heading activity-history-heading">
          <div>
            <p className="eyebrow">Most recent first</p>
            <h2 id="activity-history-title">Chronological activity</h2>
          </div>
          <p>{dashboard.activity.length} local events</p>
        </div>
        <ol className="activity-list">
          {dashboard.activity.length === 0 && (
            <li className="empty-state">No local sync or digest activity has run yet.</li>
          )}
          {dashboard.activity.map((entry) => (
            <li key={entry.id}>
              <span className={`activity-marker ${entry.status}`} aria-hidden="true" />
              <div>
                <strong>{entry.message}</strong>
                <span>
                  {entry.kind} · {formatDate(entry.occurredAt)}
                </span>
              </div>
              <span className={`activity-state ${entry.status}`}>
                {entry.status === 'partial'
                  ? 'partial · more may remain'
                  : statusLabel(entry.status)}
              </span>
            </li>
          ))}
        </ol>
      </section>
    </>
  );
}

function SettingsView({
  dashboard,
  busy,
  zoomScale,
  onZoomScaleChange,
  onSave,
  onReset,
  onExport,
  onExportOpml,
  onRestore,
}: {
  dashboard: Dashboard;
  busy: boolean;
  zoomScale: ZoomScale;
  onZoomScaleChange: (zoom: ZoomScale) => void;
  onSave: (settings: Settings) => Promise<boolean>;
  onReset: () => Promise<boolean>;
  onExport: () => Promise<boolean>;
  onExportOpml: () => Promise<boolean>;
  onRestore: () => Promise<boolean>;
}) {
  const [settings, setSettings] = useState(dashboard.settings);
  const [scheduleHour, setScheduleHour] = useState(String(dashboard.settings.scheduleHour));
  const [quietStart, setQuietStart] = useState(String(dashboard.settings.quietHoursStart));
  const [quietEnd, setQuietEnd] = useState(String(dashboard.settings.quietHoursEnd));
  const [retentionDays, setRetentionDays] = useState(String(dashboard.settings.retentionDays));
  const [installedModels, setInstalledModels] = useState<string[]>([]);
  const [restoreDialogOpen, setRestoreDialogOpen] = useState(false);
  const restoreDialogRef = useRef<HTMLDivElement>(null);
  const restoreConfirmRef = useRef<HTMLButtonElement>(null);
  const restoreReturnFocusRef = useRef<HTMLElement | null>(null);
  const update = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setSettings((current) => ({ ...current, [key]: value }));
  const scheduleNumber = /^\d{1,2}$/.test(scheduleHour) ? Number(scheduleHour) : Number.NaN;
  const quietStartNumber = /^\d{1,2}$/.test(quietStart) ? Number(quietStart) : Number.NaN;
  const quietEndNumber = /^\d{1,2}$/.test(quietEnd) ? Number(quietEnd) : Number.NaN;
  const retentionNumber = /^\d{1,3}$/.test(retentionDays) ? Number(retentionDays) : Number.NaN;
  const validHour = (value: number) => Number.isInteger(value) && value >= 0 && value <= 23;
  const scheduleValid = validHour(scheduleNumber);
  const quietStartValid = validHour(quietStartNumber);
  const quietEndValid = validHour(quietEndNumber);
  const retentionValid =
    Number.isInteger(retentionNumber) && retentionNumber >= 1 && retentionNumber <= 365;
  const modelValid = /^$|^[A-Za-z0-9._:/-]+$/.test(settings.selectedModel);
  const inQuietHours = (hour: number, start: number, end: number) =>
    start !== end && (start < end ? hour >= start && hour < end : hour >= start || hour < end);
  const scheduleConflict =
    settings.scheduleEnabled &&
    scheduleValid &&
    quietStartValid &&
    quietEndValid &&
    inQuietHours(scheduleNumber, quietStartNumber, quietEndNumber);
  const candidate = {
    ...settings,
    feedbackCount: dashboard.settings.feedbackCount,
    scheduleHour: scheduleValid ? scheduleNumber : -1,
    quietHoursStart: quietStartValid ? quietStartNumber : -1,
    quietHoursEnd: quietEndValid ? quietEndNumber : -1,
    retentionDays: retentionValid ? retentionNumber : 0,
  };
  const settingsValid =
    SettingsSchema.safeParse(candidate).success && !scheduleConflict && modelValid;
  const openRestoreDialog = () => {
    restoreReturnFocusRef.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setRestoreDialogOpen(true);
  };
  const closeRestoreDialog = (restoreFocus = true) => {
    setRestoreDialogOpen(false);
    const returnTarget = restoreReturnFocusRef.current;
    restoreReturnFocusRef.current = null;
    if (restoreFocus) window.requestAnimationFrame(() => returnTarget?.focus());
  };
  const trapRestoreDialogFocus = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Tab') return;
    const focusable = restoreDialogRef.current?.querySelectorAll<HTMLElement>(
      'button:not([disabled]), input:not([disabled]), [href], [tabindex]:not([tabindex="-1"])',
    );
    if (!focusable?.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };
  useEffect(() => {
    if (!restoreDialogOpen) return;
    const frame = window.requestAnimationFrame(() => restoreConfirmRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [restoreDialogOpen]);
  useEffect(() => {
    if (!restoreDialogOpen) return;
    const dismissWithEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busy) {
        event.preventDefault();
        closeRestoreDialog();
      }
    };
    document.addEventListener('keydown', dismissWithEscape);
    return () => document.removeEventListener('keydown', dismissWithEscape);
  }, [busy, restoreDialogOpen]);
  return (
    <>
      <PageHeader
        eyebrow="Local-first by default"
        title="Privacy & settings."
        detail="See what runs, what leaves the computer, and how long local copies remain."
      />
      <section className="settings-grid">
        <article className="settings-panel">
          <h2>Reading scale</h2>
          <p>
            Make the whole app easier to read without changing the source material. This preference
            stays on this device.
          </p>
          <fieldset className="reading-scale-control" aria-describedby="reading-scale-help">
            <legend>Page scale</legend>
            {ZOOM_SCALES.map((scale) => (
              <label key={scale}>
                <input
                  type="radio"
                  name="reading-scale"
                  value={scale}
                  checked={zoomScale === scale}
                  onChange={() => onZoomScaleChange(scale)}
                />
                <span>{Math.round(scale * 100)}%</span>
              </label>
            ))}
          </fieldset>
          <p id="reading-scale-help" className="hint">
            You can also use {zoomShortcutLabel()} to change page zoom with your current platform.
          </p>
        </article>
        <article className="settings-panel">
          <h2>Backup &amp; recovery</h2>
          <p>
            Export creates a consistent local SQLite snapshot. Restore validates and migrates a copy
            before replacing this device’s data; the prior state is retained as a local rollback
            snapshot if replacement fails.
          </p>
          <div className="library-actions">
            <button className="secondary" disabled={busy} onClick={() => void onExportOpml()}>
              Export RSS as OPML
            </button>
            <button className="secondary" disabled={busy} onClick={() => void onExport()}>
              Export local backup
            </button>
            <button className="secondary" disabled={busy} onClick={openRestoreDialog}>
              Restore backup
            </button>
          </div>
        </article>
        <article className="settings-panel">
          <h2>Finite editions</h2>
          <label htmlFor="edition-size">Items per edition</label>
          <input
            id="edition-size"
            type="range"
            min="10"
            max="40"
            value={settings.editionSize}
            onChange={(event) => update('editionSize', Number(event.target.value))}
          />
          <p className="hint">
            <strong>{settings.editionSize} items maximum.</strong> Web first reserves room across
            sources, then fills the remaining finite space with your explicit preferences and a
            chronological reserve.
          </p>
          <label className="toggle-row">
            <span>
              <strong>Close to tray</strong>
              <small>
                Closing the window keeps the local scheduler available until you choose Quit from
                the tray.
              </small>
            </span>
            <input
              type="checkbox"
              checked={settings.closeToTray}
              onChange={(event) => update('closeToTray', event.target.checked)}
            />
          </label>
        </article>
        <article className="settings-panel">
          <h2>Edition schedule</h2>
          <label className="toggle-row">
            <span>
              <strong>Scheduled editions</strong>
              <small>
                {dashboard.runner.active
                  ? 'Runs only while Web is open; at most one missed edition catches up'
                  : 'Unavailable in browser preview or when the Rust runner is stopped'}
              </small>
            </span>
            <input
              type="checkbox"
              checked={settings.scheduleEnabled}
              disabled={!dashboard.runner.active}
              onChange={(event) => update('scheduleEnabled', event.target.checked)}
            />
          </label>
          <label htmlFor="schedule-hour">
            Prepare at{' '}
            <input
              id="schedule-hour"
              type="number"
              min="0"
              max="23"
              value={scheduleHour}
              aria-invalid={!scheduleValid || scheduleConflict}
              aria-describedby="schedule-hour-error"
              onChange={(event) => setScheduleHour(event.target.value)}
            />
            :00 local time
          </label>
          <span id="schedule-hour-error" className="field-message" role="status">
            {!scheduleValid
              ? 'Enter a whole hour from 0 through 23.'
              : scheduleConflict
                ? 'Choose a preparation hour outside quiet hours.'
                : 'Use local time, from 0 through 23.'}
          </span>
          <fieldset className="quiet-hours">
            <legend>Quiet hours (local time)</legend>
            <label htmlFor="quiet-start">
              Start
              <input
                id="quiet-start"
                type="number"
                min="0"
                max="23"
                value={quietStart}
                aria-invalid={!quietStartValid}
                aria-describedby="quiet-start-error"
                onChange={(event) => setQuietStart(event.target.value)}
              />
            </label>
            <label htmlFor="quiet-end">
              End
              <input
                id="quiet-end"
                type="number"
                min="0"
                max="23"
                value={quietEnd}
                aria-invalid={!quietEndValid}
                aria-describedby="quiet-end-error"
                onChange={(event) => setQuietEnd(event.target.value)}
              />
            </label>
            <small>
              Hours are start-inclusive and end-exclusive. Matching hours disable the quiet window.
            </small>
            <span id="quiet-start-error" className="field-message">
              {quietStartValid
                ? 'Start uses a whole hour from 0 through 23.'
                : 'Enter a whole start hour from 0 through 23.'}
            </span>
            <span id="quiet-end-error" className="field-message">
              {quietEndValid
                ? 'End uses a whole hour from 0 through 23.'
                : 'Enter a whole end hour from 0 through 23.'}
            </span>
          </fieldset>
          <dl className="host-capabilities">
            <div>
              <dt>Last attempt</dt>
              <dd>{formatDate(dashboard.runner.lastAttemptAt)}</dd>
            </div>
            <div>
              <dt>Last success</dt>
              <dd>{formatDate(dashboard.runner.lastSuccessAt)}</dd>
            </div>
            <div>
              <dt>Next actual eligible execution</dt>
              <dd>{formatDate(dashboard.runner.nextScheduledAt)}</dd>
            </div>
            <div>
              <dt>State</dt>
              <dd>
                {dashboard.runner.inFlight
                  ? 'Running'
                  : dashboard.runner.active
                    ? `Waiting while app is open · last ${dashboard.runner.lastOutcome}`
                    : 'Inactive'}
              </dd>
            </div>
          </dl>
          <p className="hint">
            {dashboard.runner.detail} Due-only resident work respects quiet hours. No hidden OS task
            or closed-app execution is installed; tray behavior remains deferred.
          </p>
        </article>
        <article className="settings-panel">
          <h2>Data retention</h2>
          <label htmlFor="retention-days">
            Keep normalized source data for{' '}
            <input
              id="retention-days"
              type="number"
              min="1"
              max="365"
              value={retentionDays}
              aria-invalid={!retentionValid}
              aria-describedby="retention-days-error"
              onChange={(event) => setRetentionDays(event.target.value)}
            />{' '}
            days
          </label>
          <span id="retention-days-error" className="field-message" role="status">
            {retentionValid
              ? 'Allowed range: 1 through 365 days.'
              : 'Enter a whole number from 1 through 365.'}
          </span>
          <label className="toggle-row">
            <span>
              <strong>Remote media</strong>
              <small>Off prevents avatars and images from loading</small>
            </span>
            <input
              type="checkbox"
              checked={settings.remoteMedia}
              onChange={(event) => update('remoteMedia', event.target.checked)}
            />
          </label>
          <p className="hint">
            Web does not encrypt its SQLite database at the application layer; it relies on your
            operating system and full-disk protection. Backups and exports may retain older copies.
          </p>
        </article>
        <article className="settings-panel model-panel">
          <h2>Local model</h2>
          <div className="model-status">
            <span
              className={`status-dot ${dashboard.model.state === 'ready' ? 'healthy' : 'attention'}`}
            />
            <div>
              <strong>
                {dashboard.model.state === 'ready'
                  ? dashboard.model.model
                  : 'Deterministic fallback active'}
              </strong>
              <small>{dashboard.model.state.replaceAll('_', ' ')}</small>
            </div>
          </div>
          <label className="model-selector" htmlFor="selected-model">
            Explicit installed Ollama model
            <input
              id="selected-model"
              list="installed-models"
              type="text"
              maxLength={200}
              value={settings.selectedModel}
              aria-invalid={!modelValid}
              aria-describedby="selected-model-help"
              placeholder="Blank uses deterministic fallback"
              onChange={(event) => update('selectedModel', event.target.value)}
            />
            <datalist id="installed-models">
              {installedModels.map((model) => (
                <option value={model} key={model} />
              ))}
            </datalist>
            <button
              type="button"
              className="text-button"
              disabled={busy}
              onClick={() => void transport.installedModels().then(setInstalledModels)}
            >
              Refresh installed models
            </button>
          </label>
          <span
            id="selected-model-help"
            className={modelValid ? 'field-message compact' : 'field-message compact error'}
          >
            {modelValid
              ? 'Use the exact installed name; letters, numbers, dot, underscore, colon, slash, and hyphen only.'
              : 'Remove spaces and @; use only letters, numbers, dot, underscore, colon, slash, and hyphen.'}
          </span>
          <p className="hint">{ollamaSetupHint()}</p>
          <p>{dashboard.model.detail}</p>
          {dashboard.model.model && (
            <dl className="host-capabilities">
              <div>
                <dt>Exact model</dt>
                <dd>{dashboard.model.model}</dd>
              </div>
              <div>
                <dt>Digest</dt>
                <dd>{dashboard.model.digest ?? 'Not reported'}</dd>
              </div>
              <div>
                <dt>Parameters / quantization</dt>
                <dd>
                  {dashboard.model.parameterSize ?? 'Unknown'} ·{' '}
                  {dashboard.model.quantization ?? 'Unknown'}
                </dd>
              </div>
              <div>
                <dt>Runtime / bytes</dt>
                <dd>
                  {dashboard.model.runtimeVersion ?? 'Unknown'} ·{' '}
                  {dashboard.model.sizeBytes?.toLocaleString() ?? 'Unknown'}
                </dd>
              </div>
            </dl>
          )}
          <h3>{dashboard.host.recommendedProfile.title} profile suggested</h3>
          <p>{dashboard.host.recommendedProfile.rationale}</p>
          <dl className="host-capabilities">
            <div>
              <dt>Host</dt>
              <dd>
                {dashboard.host.os} · {dashboard.host.arch}
              </dd>
            </div>
            <div>
              <dt>Memory / CPU</dt>
              <dd>
                {dashboard.host.totalMemoryGb > 0
                  ? `${dashboard.host.totalMemoryGb} GB total / ${dashboard.host.availableMemoryGb} GB available`
                  : 'Unknown'}{' '}
                · {dashboard.host.logicalCpuCount || 'unknown'} logical CPUs
              </dd>
            </div>
            <div>
              <dt>Suggested model</dt>
              <dd>{dashboard.host.recommendedProfile.generationModel}</dd>
            </div>
            <div>
              <dt>Context / concurrency</dt>
              <dd>
                {dashboard.host.recommendedProfile.contextWindow.toLocaleString()} tokens · 1
                request
              </dd>
            </div>
          </dl>
          <p className="hint">
            Accelerator, battery, and network-cost state remain conservative when the host cannot
            report them reliably. Recommendations never download a model or enable cloud use.
          </p>
          <ul>
            <li>Loopback endpoint with proxy bypass</li>
            <li>No cloud fallback or automatic model download</li>
            <li>
              At most six new items per whole sync use the ready selected model; every failure falls
              back extractively
            </li>
          </ul>
        </article>
        <article className="settings-panel">
          <h2>How importance works</h2>
          <p>
            Web stores only feedback you deliberately provide. More/Less adjust a bounded per-source
            weight once that source has at least 3 active signals; below that threshold, ranking
            stays chronological. At least a quarter of every edition&rsquo;s slots are always
            chronological, immune to that weighting. Each item&rsquo;s &ldquo;why shown&rdquo; note
            names the exact reason. Web does not collect dwell time, scrolling, opens, or
            notification clicks.
          </p>
          <p>
            <strong>{dashboard.settings.feedbackCount}</strong> explicit feedback signals stored
            locally.
          </p>
          <label className="toggle-row">
            <span>
              <strong>Pause learned ranking</strong>
              <small>
                When on, every edition is ordered purely by publish time regardless of stored
                feedback
              </small>
            </span>
            <input
              type="checkbox"
              checked={settings.rankingPaused}
              onChange={(event) => update('rankingPaused', event.target.checked)}
            />
          </label>
          <button
            className="secondary"
            type="button"
            disabled={busy || dashboard.settings.feedbackCount === 0}
            onClick={() => void onReset()}
          >
            Reset learning
          </button>
        </article>
      </section>
      <div className="save-bar">
        <span aria-live="polite">
          {settingsValid
            ? 'Settings stay on this computer.'
            : 'Correct the highlighted settings before saving.'}
        </span>
        <button
          className="primary"
          disabled={busy || !settingsValid}
          onClick={() => void onSave(candidate as Settings)}
        >
          {busy ? 'Saving…' : 'Save settings'}
        </button>
      </div>
      {restoreDialogOpen && (
        <div
          className="command-backdrop source-dialog-backdrop"
          onMouseDown={(event) => {
            if (event.target === event.currentTarget && !busy) closeRestoreDialog();
          }}
        >
          <div
            ref={restoreDialogRef}
            className="command-dialog source-action-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby="restore-backup-title"
            aria-describedby="restore-backup-description"
            onKeyDown={trapRestoreDialogFocus}
          >
            <div className="command-dialog-header">
              <div>
                <p className="eyebrow">Local data</p>
                <h2 id="restore-backup-title">Restore a local backup?</h2>
              </div>
              <button
                className="secondary command-close"
                type="button"
                disabled={busy}
                onClick={() => closeRestoreDialog()}
              >
                Close
              </button>
            </div>
            <div className="source-action-body">
              <p id="restore-backup-description">
                Web will let you choose a backup, validate and migrate a copy, then replace this
                device&rsquo;s local data. If replacement or reopening fails, it retains a rollback
                snapshot of the prior state.
              </p>
              <p className="source-delete-warning">
                This replaces the current device data. Export a backup first if you may need the
                current state later.
              </p>
              <div className="source-dialog-actions">
                <button
                  className="secondary"
                  type="button"
                  disabled={busy}
                  onClick={() => closeRestoreDialog()}
                >
                  Keep current data
                </button>
                <button
                  ref={restoreConfirmRef}
                  className="danger-button"
                  type="button"
                  disabled={busy}
                  onClick={() =>
                    void onRestore().then((restored) => {
                      if (restored) closeRestoreDialog(false);
                    })
                  }
                >
                  Choose backup and restore
                </button>
              </div>
            </div>
          </div>
        </div>
      )}
    </>
  );
}

export default App;
