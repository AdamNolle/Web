import { useCallback, useEffect, useId, useRef, useState } from 'react';
import { transport } from './transport';
import {
  SettingsSchema,
  type Dashboard,
  type DigestItem,
  type FeedbackSignal,
  type Settings,
  type SyncOutcome,
  type Source,
} from './types';

type View = 'today' | 'trends' | 'sources' | 'activity' | 'settings';
const views: Array<{ id: View; label: string }> = [
  { id: 'today', label: 'Today' },
  { id: 'trends', label: 'Trends' },
  { id: 'sources', label: 'Sources' },
  { id: 'activity', label: 'Activity' },
  { id: 'settings', label: 'Privacy & settings' },
];

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
  const mainRef = useRef<HTMLElement>(null);
  const dashboardRef = useRef<Dashboard | undefined>(undefined);
  const pendingDashboardRef = useRef<Dashboard | undefined>(undefined);
  const highestPrivacyEpochRef = useRef(0);
  const mutationGenerationRef = useRef(0);
  const undoButtonRef = useRef<HTMLButtonElement>(null);
  const feedbackOriginRef = useRef<{ itemId: string; signal: FeedbackSignal } | undefined>(
    undefined,
  );

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
        setNotice('Edition ready');
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

  if (!dashboard) {
    return (
      <main className="loading" aria-live="polite">
        <div className="brand-mark" aria-hidden="true" />
        <p>{notice}</p>
        {loadFailed && (
          <button className="primary" onClick={() => void loadDashboard()}>
            Retry local load
          </button>
        )}
      </main>
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
          />
        )}
        {view === 'trends' && <Trends dashboard={dashboard} />}
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
          />
        )}
      </main>

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
}: {
  dashboard: Dashboard;
  busy: boolean;
  onRefresh: () => void;
  onSync: () => void;
  onFeedback: (itemId: string, signal: FeedbackSignal) => void;
}) {
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
}: {
  item: DigestItem;
  featured: boolean;
  busy: boolean;
  onFeedback: (itemId: string, signal: FeedbackSignal) => void;
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
      <div className="generated-label">
        {item.summaryMethod === 'model'
          ? `Locally generated · ${item.summaryProvider}`
          : item.summaryMethod === 'extractive'
            ? 'Extractive local fallback'
            : 'Demonstration fixture'}
      </div>
      <p className="summary">{item.summary}</p>
      <div className="comment-overview">
        <strong>
          Conversation overview ·{' '}
          {item.summaryMethod === 'model'
            ? 'locally generated'
            : item.summaryMethod === 'extractive'
              ? 'extractive fallback'
              : 'demonstration fixture'}
        </strong>
        <p>{item.commentOverview}</p>
        {item.summaryUncertainty && <small>{item.summaryUncertainty}</small>}
      </div>
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
          {item.evidence.map((evidence, index) => (
            <blockquote key={`${evidence.source}-${evidence.publishedAt}-${index}`}>
              <p>“{evidence.excerpt}”</p>
              <footer>
                {evidence.author} · {evidence.source} · {timestampLabel(evidence.timestampKind)}{' '}
                {formatDate(evidence.publishedAt)}
                <br />
                {evidence.canonicalUrl ? (
                  <>
                    <code>{evidence.canonicalUrl}</code>
                    <span className="source-url-note">
                      Copyable source URL; external opening is not enabled.
                    </span>
                  </>
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
          <article className="trend-card" key={trend.id}>
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

function Sources({
  dashboard,
  busy,
  onAdd,
  onDelete,
  statusMessage,
  statusIsError,
}: {
  dashboard: Dashboard;
  busy: boolean;
  onAdd: (label: string, url: string) => Promise<boolean>;
  onDelete: (source: Source) => void;
  statusMessage: string;
  statusIsError: boolean;
}) {
  const [label, setLabel] = useState('');
  const [url, setUrl] = useState('');
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
        <p>
          Web fetches at most 2 MB and 100 items, blocks private-network targets, follows no more
          than three validated redirects, and stores excerpts with attribution.
        </p>
        <p
          id="rss-operation-status"
          className={statusIsError ? 'field-message error' : 'field-message'}
        >
          {statusMessage}
        </p>
      </form>
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
          <article className="source-card" key={source.id}>
            <div className="source-icon" aria-hidden="true">
              {source.kind.slice(0, 1).toUpperCase()}
            </div>
            <div>
              <div className="source-title">
                <h2>{source.label}</h2>
                <span className={`source-status ${source.status}`}>
                  <span className={`status-dot ${source.status}`} />
                  {source.status}
                </span>
              </div>
              <p>{source.detail}</p>
              {source.healthDetail && <p className="hint">{source.healthDetail}</p>}
              <dl>
                <div>
                  <dt>Last sync</dt>
                  <dd>{formatDate(source.lastSync)}</dd>
                </div>
                <div>
                  <dt>Next eligible poll / retry</dt>
                  <dd>{source.nextSync === null ? 'Eligible now' : formatDate(source.nextSync)}</dd>
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
                  <dt>Last page</dt>
                  <dd>
                    {source.syncFinality === 'partial' ? 'Partial · more may remain' : 'Complete'}
                  </dd>
                </div>
              </dl>
            </div>
            <button
              className="danger-text"
              disabled={busy}
              onClick={() => {
                if (
                  window.confirm(
                    `Delete ${source.label} and all of its local posts, summaries, feedback, and credentials? This cannot be undone.`,
                  )
                )
                  onDelete(source);
              }}
            >
              Disconnect & delete
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
    </>
  );
}

function Activity({ dashboard }: { dashboard: Dashboard }) {
  return (
    <>
      <PageHeader
        eyebrow="Quietly accountable"
        title="Activity."
        detail="A concise local history of sync, model, and schedule work. Post text, prompts, credentials, and private URLs are never logged here."
      />
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
              {entry.status === 'partial' ? 'partial · more may remain' : entry.status}
            </span>
          </li>
        ))}
      </ol>
    </>
  );
}

function SettingsView({
  dashboard,
  busy,
  onSave,
  onReset,
}: {
  dashboard: Dashboard;
  busy: boolean;
  onSave: (settings: Settings) => Promise<boolean>;
  onReset: () => Promise<boolean>;
}) {
  const [settings, setSettings] = useState(dashboard.settings);
  const [scheduleHour, setScheduleHour] = useState(String(dashboard.settings.scheduleHour));
  const [quietStart, setQuietStart] = useState(String(dashboard.settings.quietHoursStart));
  const [quietEnd, setQuietEnd] = useState(String(dashboard.settings.quietHoursEnd));
  const [retentionDays, setRetentionDays] = useState(String(dashboard.settings.retentionDays));
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
  return (
    <>
      <PageHeader
        eyebrow="Local-first by default"
        title="Privacy & settings."
        detail="See what runs, what leaves the computer, and how long local copies remain."
      />
      <section className="settings-grid">
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
              type="text"
              maxLength={200}
              value={settings.selectedModel}
              aria-invalid={!modelValid}
              aria-describedby="selected-model-help"
              placeholder="Blank uses deterministic fallback"
              onChange={(event) => update('selectedModel', event.target.value)}
            />
          </label>
          <span
            id="selected-model-help"
            className={modelValid ? 'field-message compact' : 'field-message compact error'}
          >
            {modelValid
              ? 'Use the exact installed name; letters, numbers, dot, underscore, colon, slash, and hyphen only.'
              : 'Remove spaces and @; use only letters, numbers, dot, underscore, colon, slash, and hyphen.'}
          </span>
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
    </>
  );
}

export default App;
