import { describe, expect, it } from 'vitest';
import { demoDashboard } from './demoData';
import { createDemoTransport, parseDashboard } from './transport';
import type { Dashboard } from './types';

describe('transport contracts', () => {
  it('accepts backend-shaped partial activity at the native Zod boundary', () => {
    const value: Dashboard = structuredClone(demoDashboard);
    const first = value.activity[0];
    if (!first) throw new Error('demo activity fixture is required');
    value.activity[0] = { ...first, status: 'partial', message: 'Partial page stored' };
    const parsed = parseDashboard(value);
    expect(parsed.activity[0]?.status).toBe('partial');
  });

  it('accepts archive imports at the native dashboard boundary', () => {
    const value: Dashboard = structuredClone(demoDashboard);
    const first = value.sources[0];
    if (!first) throw new Error('demo source fixture is required');
    value.sources[0] = {
      ...first,
      id: 'import-x',
      kind: 'archive_import',
      detail: 'One-time local archive import.',
      nextSync: null,
    };

    const parsed = parseDashboard(value);
    expect(parsed.sources[0]?.kind).toBe('archive_import');
  });

  it('makes identical feedback retries idempotent and rejects changed payloads', async () => {
    const transport = createDemoTransport();
    const first = await transport.recordFeedback('request-a', 'post-local-ai', 'more_like_this');
    const replay = await transport.recordFeedback('request-a', 'post-local-ai', 'more_like_this');
    expect(first.settings.feedbackCount).toBe(1);
    expect(replay.settings.feedbackCount).toBe(1);
    await expect(
      transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant'),
    ).rejects.toThrow(/different feedback/);
  });

  it('keeps completed receipt tombstones across reset and makes undo after reset a no-op', async () => {
    const transport = createDemoTransport();
    await transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant');
    await transport.resetLearning('reset-a');
    const delayed = await transport.recordFeedback('request-a', 'post-local-ai', 'not_relevant');
    expect(delayed.items.some((item) => item.id === 'post-local-ai')).toBe(true);
    const undone = await transport.undoFeedback('request-a');
    expect(undone.items.some((item) => item.id === 'post-local-ai')).toBe(true);
    expect(undone.settings.feedbackCount).toBe(0);
  });

  it('keeps archive import native-only in the browser demonstration', async () => {
    const transport = createDemoTransport();
    const before = await transport.getDashboard();
    const result = await transport.importArchive('import-a', 'x', 'My X archive');

    expect(result).toMatchObject({
      status: 'canceled',
      sourceId: null,
      importedItems: 0,
      skippedItems: 0,
      changedItems: 0,
    });
    expect(result.dashboard).toEqual(before);
  });

  it('does not pretend an original opened in the browser demonstration', async () => {
    const transport = createDemoTransport();
    await expect(transport.openOriginal('https://example.com/original')).rejects.toThrow(
      /native desktop app/i,
    );
  });
});
