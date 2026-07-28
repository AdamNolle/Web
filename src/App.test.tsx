import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { demoDashboard } from './demoData';
import { createDemoTransport, setTransportForTests, type AppTransport } from './transport';
import type { Dashboard } from './types';

beforeEach(() => setTransportForTests(createDemoTransport()));

describe('calm dashboard', () => {
  it('renders a finite edition with provenance and a natural end', async () => {
    render(<App />);
    expect(await screen.findByRole('heading', { name: 'Good morning.' })).toBeInTheDocument();
    expect(screen.getAllByRole('article')).toHaveLength(4);
    expect(screen.getByRole('heading', { name: 'You’re caught up.' })).toBeInTheDocument();
    expect(screen.queryByText(/infinite/i)).not.toBeInTheDocument();

    await userEvent.click(
      screen.getByRole('button', { name: /Show evidence for Smaller local models/ }),
    );
    expect(screen.getByText(/The largest gains came from better task framing/)).toBeInTheDocument();
    expect(screen.getByText('https://example.com/local-models')).toBeInTheDocument();
  });

  it('uses explicit reversible feedback instead of passive behavior', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    const target = screen
      .getByRole('heading', { name: /Smaller local models/ })
      .closest('article')!;
    const hide = within(target).getByRole('button', { name: 'Not relevant' });
    await userEvent.click(hide);
    expect(await screen.findByText('Feedback saved.')).toBeInTheDocument();
    const undo = screen.getByRole('button', { name: 'Undo' });
    expect(undo).toHaveFocus();
    await userEvent.click(undo);
    const restored = await screen.findByRole('heading', { name: /Smaller local models/ });
    expect(restored).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getAllByRole('button', { name: 'Not relevant' })[0]).toHaveFocus(),
    );
  });

  it('does not announce feedback success when persistence fails', async () => {
    const base = createDemoTransport();
    const failing = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'recordFeedback') {
          return async () => Promise.reject(new Error('Feedback could not be stored locally.'));
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(failing);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getAllByRole('button', { name: 'More like this' })[0]!);
    expect(await screen.findByText('Feedback could not be stored locally.')).toBeInTheDocument();
    expect(screen.queryByText('Feedback saved.')).not.toBeInTheDocument();
  });

  it('shows fail-closed unknown feedback finality without offering Undo', async () => {
    const base = createDemoTransport();
    const unknown = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'recordFeedback') {
          return async () =>
            Promise.reject({
              code: 'CONFLICT',
              message:
                'That earlier feedback request has unknown finality and was not reported as saved. Refresh before choosing again.',
              retryable: false,
              correlationId: 'test-unknown',
            });
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(unknown);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getAllByRole('button', { name: 'Not relevant' })[0]!);
    const unknownMessages = await screen.findAllByText(
      /unknown finality and was not reported as saved/i,
    );
    expect(unknownMessages.some((message) => message.classList.contains('visible-status'))).toBe(
      true,
    );
    expect(screen.queryByText('Feedback saved.')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Undo' })).not.toBeInTheDocument();
  });

  it('resets explicit learning through a working control', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getAllByRole('button', { name: 'More like this' })[0]!);
    await screen.findByText('Feedback saved.');
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    const reset = screen.getByRole('button', { name: 'Reset learning' });
    expect(reset).toBeEnabled();
    await userEvent.click(reset);
    await screen.findByText('Resetting local learning complete');
    expect(reset).toHaveFocus();
    const learningPanel = screen
      .getByRole('heading', { name: 'How importance works' })
      .closest('article')!;
    expect(within(learningPanel).getByText('0')).toBeInTheDocument();
  });

  it('explains local model and connector boundaries', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    expect(screen.getByRole('heading', { name: 'Privacy & settings.' })).toBeInTheDocument();
    expect(screen.getByText(/Loopback endpoint with proxy bypass/)).toBeInTheDocument();
    expect(screen.getByLabelText(/Explicit installed Ollama model/)).toHaveValue('');
    expect(screen.getByText(/does not collect dwell time/)).toBeInTheDocument();

    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    expect(screen.getByText(/does not disguise automation/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Add read-only feed' })).toBeInTheDocument();
  });

  it('explains the bounded learned-ranking mechanics instead of claiming ranking is inactive', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    expect(screen.getByText(/at least 3 active signals/)).toBeInTheDocument();
    expect(
      screen.getByText(/quarter of every edition.s slots are always chronological/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/ranking is not active/i)).not.toBeInTheDocument();
  });

  it('pauses learned ranking through a working control that persists across views', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    const pause = screen.getByRole('checkbox', { name: /Pause learned ranking/ });
    expect(pause).not.toBeChecked();
    await userEvent.click(pause);
    await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Saving private settings complete');

    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    expect(screen.getByRole('checkbox', { name: /Pause learned ranking/ })).toBeChecked();
  });

  it('announces settings validation without coercing a cleared field', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    const schedule = screen.getByLabelText(/Prepare at/);
    await userEvent.clear(schedule);
    expect(schedule).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByText(/Enter a whole hour from 0 through 23/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  });

  it('announces a partial deliberate override with routes to source details', async () => {
    const base = createDemoTransport();
    const partial = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'syncSources') {
          return async () => ({
            dashboard: await target.getDashboard(),
            outcome: {
              mode: 'manual_override' as const,
              finality: 'partial' as const,
              changedSources: 1,
              unchangedSources: 1,
              failedSources: 1,
              changedItems: 2,
              sourceLimitReached: false,
            },
          });
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(partial);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: /Sync all now/ }));
    expect(await screen.findAllByText(/Synchronization completed partially/)).toHaveLength(2);
    expect(screen.getByRole('button', { name: 'Review sources' })).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Review activity' }));
    expect(screen.getByRole('heading', { name: 'Activity.' })).toBeInTheDocument();
  });

  it('keeps an open edition stable until an autonomous update is applied', async () => {
    const base = createDemoTransport();
    let reads = 0;
    const updating = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            const value = await target.getDashboard();
            reads += 1;
            if (reads > 1) {
              value.edition.id = 'edition-autonomous';
              value.edition.summary = 'Autonomous edition summary.';
            }
            return value;
          };
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(updating);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    expect(screen.queryByText('Autonomous edition summary.')).not.toBeInTheDocument();
    const apply = await screen.findByRole('button', { name: 'Apply new edition' });
    expect(screen.queryByText('Autonomous edition summary.')).not.toBeInTheDocument();
    await userEvent.click(apply);
    expect(await screen.findByText('Autonomous edition summary.')).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Good morning.' })).toHaveFocus(),
    );
  });

  it('purges privacy-invalidated content without applying harmless pending reordering', async () => {
    const base = createDemoTransport();
    let reads = 0;
    const removedTitle = demoDashboard.items[0]!.title;
    const retainedOverview = demoDashboard.items[1]!.commentOverview;
    const refreshedOverview = 'Refreshed bounded comment evidence.';
    const updating = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            const value = await target.getDashboard();
            reads += 1;
            if (reads > 1) {
              value.edition.id = 'edition-after-retention';
              value.edition.summary = 'Pending after retention.';
              value.privacyEpoch += 1;
              value.items = value.items
                .filter((item) => item.title !== removedTitle)
                .map((item) =>
                  item.id === demoDashboard.items[1]!.id
                    ? { ...item, commentOverview: refreshedOverview }
                    : item,
                )
                .reverse();
            }
            return value;
          };
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(updating);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await screen.findByRole('button', { name: 'Apply new edition' });
    expect(screen.queryByText(removedTitle)).not.toBeInTheDocument();
    expect(screen.queryByText(retainedOverview)).not.toBeInTheDocument();
    expect(screen.getByText(refreshedOverview)).toBeInTheDocument();
    expect(screen.queryByText('Pending after retention.')).not.toBeInTheDocument();
  });

  it('keeps a pending edition through unrelated feedback mutations', async () => {
    const base = createDemoTransport();
    let reads = 0;
    const autonomous = () => {
      const value = structuredClone(demoDashboard);
      value.edition.id = 'edition-pending-feedback';
      value.edition.summary = 'Pending feedback edition.';
      return value;
    };
    const updating = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            reads += 1;
            return reads > 1 ? autonomous() : target.getDashboard();
          };
        }
        if (property === 'recordFeedback') return async () => autonomous();
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(updating);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await screen.findByRole('button', { name: 'Apply new edition' });
    await userEvent.click(screen.getAllByRole('button', { name: 'More like this' })[0]!);
    expect(await screen.findByText('Feedback saved.')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Apply new edition' })).toBeInTheDocument();
    expect(screen.queryByText('Pending feedback edition.')).not.toBeInTheDocument();
  });

  it('rejects an older poll that resolves after Not relevant raises privacy state', async () => {
    const base = createDemoTransport();
    const stale = structuredClone(demoDashboard);
    const removed = demoDashboard.items[0]!;
    let reads = 0;
    let resolvePoll: ((value: Dashboard) => void) | undefined;
    const ordered = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            reads += 1;
            if (reads === 1) return target.getDashboard();
            return new Promise<Dashboard>((resolve) => {
              resolvePoll = resolve;
            });
          };
        }
        if (property === 'recordFeedback') {
          return async () => {
            const fresh = structuredClone(demoDashboard);
            fresh.privacyEpoch = 1;
            fresh.items = fresh.items.filter((item) => item.id !== removed.id);
            return fresh;
          };
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(ordered);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await waitFor(() => expect(resolvePoll).toBeDefined());
    const card = screen.getByRole('heading', { name: removed.title }).closest('article')!;
    await userEvent.click(within(card).getByRole('button', { name: 'Not relevant' }));
    await waitFor(() => expect(screen.queryByText(removed.title)).not.toBeInTheDocument());
    await act(async () => {
      resolvePoll?.(stale);
      await Promise.resolve();
    });
    expect(screen.queryByText(removed.title)).not.toBeInTheDocument();
  });

  it('rejects an older poll that resolves after source deletion raises privacy state', async () => {
    const base = createDemoTransport();
    const stale = structuredClone(demoDashboard);
    const removed = demoDashboard.items[0]!;
    let reads = 0;
    let resolvePoll: ((value: Dashboard) => void) | undefined;
    const ordered = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            reads += 1;
            if (reads === 1) return target.getDashboard();
            return new Promise<Dashboard>((resolve) => {
              resolvePoll = resolve;
            });
          };
        }
        if (property === 'deleteSource') {
          return async () => {
            const fresh = structuredClone(demoDashboard);
            fresh.privacyEpoch = 1;
            fresh.sources = fresh.sources.filter((source) => source.id !== removed.sourceId);
            fresh.items = fresh.items.filter((item) => item.sourceId !== removed.sourceId);
            return fresh;
          };
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(ordered);
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await waitFor(() => expect(resolvePoll).toBeDefined());
    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    const sourceCard = screen
      .getByRole('heading', { name: 'Practical AI Notes' })
      .closest('article')!;
    await userEvent.click(within(sourceCard).getByRole('button', { name: 'Disconnect & delete' }));
    await userEvent.click(screen.getByRole('button', { name: 'Today' }));
    await waitFor(() => expect(screen.queryByText(removed.title)).not.toBeInTheDocument());
    await act(async () => {
      resolvePoll?.(stale);
      await Promise.resolve();
    });
    expect(screen.queryByText(removed.title)).not.toBeInTheDocument();
  });

  it.each([
    ['settings', 'updateSettings'],
    ['add', 'addRssSource'],
    ['delete', 'deleteSource'],
    ['reset', 'resetLearning'],
  ] as const)('keeps a pending edition through unrelated %s mutations', async (kind, method) => {
    const configured = structuredClone(demoDashboard);
    if (kind === 'reset') configured.settings.feedbackCount = 1;
    const pending = () => {
      const value = structuredClone(configured);
      value.edition.id = `pending-${kind}`;
      value.edition.summary = `Pending ${kind} edition.`;
      return value;
    };
    let reads = 0;
    const base = createDemoTransport();
    const updating = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') {
          return async () => {
            reads += 1;
            return reads > 1 ? pending() : structuredClone(configured);
          };
        }
        if (property === method) return async () => pending();
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(updating);
    if (kind === 'delete') vi.spyOn(window, 'confirm').mockReturnValue(true);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await screen.findByRole('button', { name: 'Apply new edition' });
    if (kind === 'settings') {
      await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
      await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
    } else if (kind === 'add') {
      await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
      await userEvent.type(screen.getByLabelText('Feed name'), 'New feed');
      await userEvent.type(screen.getByLabelText('RSS or Atom URL'), 'https://example.test/feed');
      await userEvent.click(screen.getByRole('button', { name: 'Add read-only feed' }));
    } else if (kind === 'delete') {
      await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
      await userEvent.click(screen.getAllByRole('button', { name: 'Disconnect & delete' })[0]!);
    } else {
      await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
      await userEvent.click(screen.getByRole('button', { name: 'Reset learning' }));
    }
    expect(screen.getByRole('button', { name: 'Apply new edition' })).toBeInTheDocument();
    expect(screen.queryByText(`Pending ${kind} edition.`)).not.toBeInTheDocument();
  });

  it('gives model syntax, schedule conflict, skip-link, cap, and eligible-now truth', async () => {
    const configured = structuredClone(demoDashboard);
    configured.runner.active = true;
    const base = createDemoTransport();
    const capped = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') return async () => structuredClone(configured);
        if (property === 'syncSources') {
          return async () => ({
            dashboard: structuredClone(configured),
            outcome: {
              mode: 'manual_override' as const,
              finality: 'partial' as const,
              changedSources: 0,
              unchangedSources: 20,
              failedSources: 0,
              changedItems: 0,
              sourceLimitReached: true,
            },
          });
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(capped);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    expect(screen.getByRole('link', { name: 'Skip to content' })).toHaveAttribute(
      'href',
      '#main-content',
    );
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    const model = screen.getByLabelText(/Explicit installed Ollama model/);
    await userEvent.type(model, 'bad model@tag');
    expect(model).toHaveAttribute('aria-invalid', 'true');
    expect(screen.getByText(/Remove spaces and @/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    await userEvent.clear(model);
    await userEvent.type(model, 'llama3.2:3b');
    expect(model).toHaveAttribute('aria-invalid', 'false');
    await userEvent.click(screen.getByRole('checkbox', { name: /Scheduled editions/ }));
    const schedule = screen.getByLabelText(/Prepare at/);
    await userEvent.clear(schedule);
    await userEvent.type(schedule, '22');
    expect(schedule).toHaveAttribute('aria-invalid', 'true');
    await userEvent.click(screen.getByRole('button', { name: 'Today' }));
    await userEvent.click(screen.getByRole('button', { name: /Sync all now/ }));
    expect(
      await screen.findByText(/source cap was reached.*Unattempted sources remain eligible/i),
    ).toBeInTheDocument();
    expect(screen.queryByText(/failed sources follow bounded retry/i)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Review sources' }));
    expect(screen.getAllByText('Eligible now').length).toBeGreaterThan(0);
  });

  it('associates a failed feed URL with its visible backend error', async () => {
    const base = createDemoTransport();
    const failing = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'addRssSource') {
          return async () => Promise.reject(new Error('The feed URL was rejected safely.'));
        }
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(failing);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    await userEvent.type(screen.getByLabelText('Feed name'), 'Rejected');
    const url = screen.getByLabelText('RSS or Atom URL');
    await userEvent.type(url, 'https://example.test/feed');
    await userEvent.click(screen.getByRole('button', { name: 'Add read-only feed' }));
    expect(await screen.findAllByText('The feed URL was rejected safely.')).toHaveLength(2);
    expect(url).toHaveAttribute('aria-invalid', 'true');
    expect(url).toHaveAttribute(
      'aria-describedby',
      expect.stringContaining('rss-operation-status'),
    );
  });

  it('exposes quiet hours and rejects a scheduled hour inside them', async () => {
    const configured = structuredClone(demoDashboard);
    configured.runner.active = true;
    const base = createDemoTransport();
    const active = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') return async () => configured;
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(active);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Privacy & settings' }));
    await userEvent.click(screen.getByRole('checkbox', { name: /Scheduled editions/ }));
    const schedule = screen.getByLabelText(/Prepare at/);
    await userEvent.clear(schedule);
    await userEvent.type(schedule, '22');
    expect(screen.getByLabelText('Start')).toHaveValue(21);
    expect(screen.getByLabelText('End')).toHaveValue(7);
    expect(screen.getByText(/Choose a preparation hour outside quiet hours/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  });

  it('shows partial activity as a calm bounded outcome', async () => {
    const dashboard = structuredClone(demoDashboard);
    const first = dashboard.activity[0];
    if (!first) throw new Error('demo activity fixture is required');
    dashboard.activity[0] = { ...first, status: 'partial', message: 'A bounded page was stored' };
    const base = createDemoTransport();
    setTransportForTests(
      new Proxy(base, {
        get(target, property, receiver) {
          if (property === 'getDashboard') return async () => structuredClone(dashboard);
          return Reflect.get(target, property, receiver);
        },
      }) as AppTransport,
    );
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Activity' }));
    const state = await screen.findByText('partial · more may remain');
    expect(state).toHaveClass('activity-state', 'partial');
  });

  it('shows social connector prerequisites without connect or credential controls', async () => {
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    expect(screen.getByRole('heading', { name: 'Official social connectors' })).toBeInTheDocument();
    expect(
      screen.getByText(/Instance OAuth compatibility and provider policy review/),
    ).toBeInTheDocument();
    expect(screen.getByText(/public HTTPS client-metadata\/policy origin/)).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: /connect mastodon|connect bluesky/i }),
    ).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/token|password|credential/i)).not.toBeInTheDocument();
  });

  it('renders an explicit production-style empty source state', async () => {
    const empty = structuredClone(demoDashboard);
    empty.sources = [];
    empty.items = [];
    empty.trends = [];
    empty.activity = [];
    const base = createDemoTransport();
    const emptyTransport = new Proxy(base, {
      get(target, property, receiver) {
        if (property === 'getDashboard') return async () => empty;
        return Reflect.get(target, property, receiver);
      },
    }) as AppTransport;
    setTransportForTests(emptyTransport);
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await userEvent.click(screen.getByRole('button', { name: 'Sources' }));
    expect(screen.getByText(/No sources are connected yet/)).toBeInTheDocument();
  });
});
