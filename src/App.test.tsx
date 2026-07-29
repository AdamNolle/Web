import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import App from './App';
import { demoDashboard } from './demoData';
import { createDemoTransport, setTransportForTests, type AppTransport } from './transport';
import type { Dashboard } from './types';

beforeEach(() => {
  setTransportForTests(createDemoTransport());
  const storedPresentationState = new Map<string, string>();
  Object.defineProperty(window, 'localStorage', {
    configurable: true,
    value: {
      getItem: (key: string) => storedPresentationState.get(key) ?? null,
      setItem: (key: string, value: string) => storedPresentationState.set(key, value),
      removeItem: (key: string) => storedPresentationState.delete(key),
      clear: () => storedPresentationState.clear(),
      key: (index: number) => [...storedPresentationState.keys()][index] ?? null,
      get length() {
        return storedPresentationState.size;
      },
    } satisfies Storage,
  });
  document.documentElement.dataset.theme = 'auto';
  document.documentElement.dataset.themeMode = 'auto';
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: undefined,
  });
});

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

  it('opens eligible HTTPS evidence through the app transport and reports success', async () => {
    const testTransport = createDemoTransport();
    const openOriginal = vi.fn(async () => undefined);
    testTransport.openOriginal = openOriginal;
    setTransportForTests(testTransport);
    const user = userEvent.setup();

    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await user.click(
      screen.getByRole('button', { name: /Show evidence for Smaller local models/ }),
    );
    await user.click(
      screen.getByRole('button', {
        name: /Open original for Smaller local models.* from Practical AI Notes/,
      }),
    );

    expect(openOriginal).toHaveBeenCalledOnce();
    expect(openOriginal).toHaveBeenCalledWith('https://example.com/local-models');
    await waitFor(() =>
      expect(
        screen.getByText('Status: Original source opened in your default browser.'),
      ).toBeInTheDocument(),
    );
  });

  it('surfaces original-opening failures through the existing notice', async () => {
    const testTransport = createDemoTransport();
    testTransport.openOriginal = vi.fn(async () => {
      throw new Error('The browser launcher is unavailable.');
    });
    setTransportForTests(testTransport);
    const user = userEvent.setup();

    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await user.click(
      screen.getByRole('button', { name: /Show evidence for Smaller local models/ }),
    );
    await user.click(
      screen.getByRole('button', {
        name: /Open original for Smaller local models.* from Practical AI Notes/,
      }),
    );

    await waitFor(() =>
      expect(screen.getByText('Status: The browser launcher is unavailable.')).toBeInTheDocument(),
    );
  });

  it('keeps HTTP and absent evidence URLs non-operable', async () => {
    const dashboard = structuredClone(demoDashboard);
    const firstItem = dashboard.items[0]!;
    const evidence = firstItem.evidence[0]!;
    firstItem.evidence = [
      { ...evidence, canonicalUrl: 'http://example.com/copy-only' },
      { ...evidence, source: 'No URL fixture', canonicalUrl: null },
    ];
    const testTransport = createDemoTransport();
    const openOriginal = vi.fn(async () => undefined);
    testTransport.getDashboard = async () => dashboard;
    testTransport.openOriginal = openOriginal;
    setTransportForTests(testTransport);
    const user = userEvent.setup();

    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await user.click(
      screen.getByRole('button', { name: /Show evidence for Smaller local models/ }),
    );

    expect(screen.getByText('http://example.com/copy-only')).toBeInTheDocument();
    expect(screen.getByText('No canonical web URL was supplied.')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /Open original/ })).not.toBeInTheDocument();
    expect(openOriginal).not.toHaveBeenCalled();
  });

  it('opens the command palette from the keyboard, navigates with arrows, and restores focus', async () => {
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    const origin = screen.getByRole('button', { name: 'Sources' });
    origin.focus();

    await user.keyboard('{Control>}k{/Control}');
    const dialog = screen.getByRole('dialog', { name: 'Command palette' });
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    const search = within(dialog).getByRole('combobox', { name: 'Search commands' });
    await waitFor(() => expect(search).toHaveFocus());
    const options = within(dialog).getAllByRole('option');
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    expect(within(dialog).getByText('Practical AI Notes')).toBeInTheDocument();
    expect(
      within(dialog).getByText('Local-first tools are narrowing the convenience gap'),
    ).toBeInTheDocument();

    await user.keyboard('{ArrowDown}');
    expect(options[1]).toHaveAttribute('aria-selected', 'true');
    expect(search).toHaveFocus();
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(origin).toHaveFocus());

    await user.keyboard('{Meta>}k{/Meta}');
    const reopened = screen.getByRole('dialog', { name: 'Command palette' });
    const reopenedSearch = within(reopened).getByRole('combobox', { name: 'Search commands' });
    await user.type(reopenedSearch, 'Trends');
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Trends, without the hype.' })).toHaveFocus(),
    );
  });

  it('persists explicit themes and follows system changes only in Auto mode', async () => {
    let prefersDark = true;
    const listeners = new Set<() => void>();
    Object.defineProperty(window, 'matchMedia', {
      configurable: true,
      value: vi.fn(() => ({
        get matches() {
          return prefersDark;
        },
        media: '(prefers-color-scheme: dark)',
        onchange: null,
        addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
        removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
        dispatchEvent: () => true,
      })),
    });
    const user = userEvent.setup();
    const rendered = render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    const themes = screen.getByRole('radiogroup', { name: 'Theme' });
    const auto = within(themes).getByRole('radio', { name: 'Auto' });
    const light = within(themes).getByRole('radio', { name: 'Light' });
    const dark = within(themes).getByRole('radio', { name: 'Dark' });
    expect(auto).toBeChecked();
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');

    prefersDark = false;
    act(() => listeners.forEach((listener) => listener()));
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');

    await user.click(dark);
    expect(window.localStorage.getItem('web.presentation.theme')).toBe('dark');
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
    expect(dark).toBeChecked();

    prefersDark = false;
    act(() => listeners.forEach((listener) => listener()));
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');

    rendered.unmount();
    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    expect(screen.getByRole('radio', { name: 'Dark' })).toBeChecked();
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
    expect(light).not.toBeInTheDocument();
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
    const retainedSummary = demoDashboard.items[1]!.summary;
    const refreshedSummary = 'Refreshed bounded summary evidence.';
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
                    ? { ...item, summary: refreshedSummary }
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
    expect(screen.queryByText(retainedSummary)).not.toBeInTheDocument();
    expect(screen.getByText(refreshedSummary)).toBeInTheDocument();
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
    expect(screen.getByRole('heading', { name: 'System snapshot' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Runner' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Model path' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Source health' })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Chronological activity' })).toBeInTheDocument();
    expect(screen.getByRole('table', { name: /Current health/ })).toBeInTheDocument();
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

  it('imports an official archive through the native picker contract', async () => {
    const user = userEvent.setup();
    const base = createDemoTransport();
    const importedDashboard: Dashboard = structuredClone(demoDashboard);
    const template = importedDashboard.sources[0];
    if (!template) throw new Error('demo source fixture is required');
    importedDashboard.sources.push({
      ...template,
      id: 'import-instagram',
      kind: 'archive_import',
      label: 'Family Instagram archive',
      detail: 'One-time local archive import.',
      nextSync: null,
      itemCount: 1,
    });
    let received: { requestId: string; platform: 'x' | 'instagram'; label: string } | undefined;
    const importArchive: AppTransport['importArchive'] = async (requestId, platform, label) => {
      received = { requestId, platform, label };
      return {
        status: 'imported',
        sourceId: 'import-instagram',
        importedItems: 1,
        skippedItems: 0,
        changedItems: 1,
        dashboard: structuredClone(importedDashboard),
      };
    };
    setTransportForTests(
      new Proxy(base, {
        get(target, property, receiver) {
          if (property === 'importArchive') return importArchive;
          return Reflect.get(target, property, receiver);
        },
      }) as AppTransport,
    );

    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    await user.click(screen.getByRole('button', { name: 'Sources' }));
    expect(
      screen.getByRole('heading', { name: 'Import an official data archive' }),
    ).toBeInTheDocument();
    expect(screen.getByText(/25,000 entries per import/)).toBeInTheDocument();
    await user.selectOptions(screen.getByLabelText('Archive platform'), 'instagram');
    const label = screen.getByLabelText('Archive name');
    await user.clear(label);
    await user.type(label, 'Family Instagram archive');
    await user.click(screen.getByRole('button', { name: 'Choose archive file' }));

    expect(
      await screen.findAllByText(/Imported 1 Instagram posts; 1 local items changed/),
    ).not.toHaveLength(0);
    expect(received).toMatchObject({
      platform: 'instagram',
      label: 'Family Instagram archive',
    });
    expect(received?.requestId).toEqual(expect.any(String));
    expect(screen.getByRole('heading', { name: 'Family Instagram archive' })).toBeInTheDocument();
    expect(screen.getByText('Manual re-import only')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Activity' }));
    const archiveHealthRow = screen.getByRole('row', { name: /Family Instagram archive/ });
    expect(within(archiveHealthRow).getByText('Manual re-import only')).toBeInTheDocument();
  });

  it('routes first-run source actions to the relevant focused controls', async () => {
    const user = userEvent.setup();
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
    expect(
      await screen.findByRole('heading', { name: 'Choose your first source.' }),
    ).toBeInTheDocument();
    expect(screen.getByText('Ready to add your first source')).toHaveClass('live-region');
    expect(screen.getByText('Status: Ready to add your first source')).toBeInTheDocument();
    expect(screen.getByText(/Source data, summaries, and feedback stay/)).toBeInTheDocument();
    expect(screen.getByText(/never posts, follows, likes/)).toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: 'You’re caught up.' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Add an RSS feed' }));
    expect(screen.getByRole('heading', { name: 'Your sources.' })).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText('Feed name')).toHaveFocus());

    await user.click(screen.getByRole('button', { name: 'Today' }));
    await user.click(screen.getByRole('button', { name: 'Import an official archive' }));
    expect(
      screen.getByRole('heading', { name: 'Import an official data archive' }),
    ).toBeInTheDocument();
    await waitFor(() => expect(screen.getByLabelText('Archive platform')).toHaveFocus());
  });

  it('keeps the normal caught-up edition when connected sources have no eligible items', async () => {
    const emptyEdition = structuredClone(demoDashboard);
    emptyEdition.items = [];
    emptyEdition.trends = [];
    const base = createDemoTransport();
    setTransportForTests(
      new Proxy(base, {
        get(target, property, receiver) {
          if (property === 'getDashboard') return async () => emptyEdition;
          return Reflect.get(target, property, receiver);
        },
      }) as AppTransport,
    );

    render(<App />);
    await screen.findByRole('heading', { name: 'Good morning.' });
    expect(screen.getByRole('heading', { name: 'You’re caught up.' })).toBeInTheDocument();
    expect(screen.getByText('0 useful items')).toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Import an official archive' }),
    ).not.toBeInTheDocument();
  });
});
