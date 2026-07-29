import { invoke } from '@tauri-apps/api/core';
import { demoDashboard } from './demoData';
import {
  ArchiveImportResultSchema,
  DashboardSchema,
  SettingsSchema,
  SyncOutcomeSchema,
  type ArchiveImportPlatform,
  type ArchiveImportResult,
  type Dashboard,
  type FeedbackSignal,
  type Settings,
  type SyncSourcesResult,
} from './types';

export interface AppTransport {
  getDashboard(): Promise<Dashboard>;
  runDigest(requestId: string): Promise<Dashboard>;
  syncSources(requestId: string): Promise<SyncSourcesResult>;
  recordFeedback(requestId: string, itemId: string, signal: FeedbackSignal): Promise<Dashboard>;
  undoFeedback(requestId: string): Promise<Dashboard>;
  updateSettings(requestId: string, settings: Settings): Promise<Dashboard>;
  addRssSource(requestId: string, label: string, url: string): Promise<Dashboard>;
  importArchive(
    requestId: string,
    platform: ArchiveImportPlatform,
    label: string,
  ): Promise<ArchiveImportResult>;
  openOriginal(url: string): Promise<void>;
  deleteSource(requestId: string, sourceId: string): Promise<Dashboard>;
  resetLearning(requestId: string): Promise<Dashboard>;
}

const isTauri = () => '__TAURI_INTERNALS__' in window;

export const parseDashboard = (value: unknown) => DashboardSchema.parse(value);

class TauriTransport implements AppTransport {
  async getDashboard() {
    return parseDashboard(await invoke('get_dashboard'));
  }
  async runDigest(requestId: string) {
    return parseDashboard(await invoke('run_digest', { request: { requestId } }));
  }
  async syncSources(requestId: string) {
    const value = await invoke('sync_sources', { request: { requestId } });
    const parsed = value as { dashboard?: unknown; outcome?: unknown };
    return {
      dashboard: parseDashboard(parsed.dashboard),
      outcome: SyncOutcomeSchema.parse(parsed.outcome),
    };
  }
  async recordFeedback(requestId: string, itemId: string, signal: FeedbackSignal) {
    return parseDashboard(
      await invoke('record_feedback', { request: { requestId, itemId, signal } }),
    );
  }
  async undoFeedback(requestId: string) {
    return parseDashboard(await invoke('undo_feedback', { request: { requestId } }));
  }
  async updateSettings(requestId: string, settings: Settings) {
    return parseDashboard(await invoke('update_settings', { request: { requestId, settings } }));
  }
  async addRssSource(requestId: string, label: string, url: string) {
    return parseDashboard(await invoke('add_rss_source', { request: { requestId, label, url } }));
  }
  async importArchive(requestId: string, platform: ArchiveImportPlatform, label: string) {
    return ArchiveImportResultSchema.parse(
      await invoke('import_archive', { request: { requestId, platform, label } }),
    );
  }
  async openOriginal(url: string) {
    await invoke('open_original', { request: { url } });
  }
  async deleteSource(requestId: string, sourceId: string) {
    return parseDashboard(await invoke('delete_source', { request: { requestId, sourceId } }));
  }
  async resetLearning(requestId: string) {
    return parseDashboard(await invoke('reset_learning', { request: { requestId } }));
  }
}

class DemoTransport implements AppTransport {
  private state = structuredClone(demoDashboard);
  private feedbackSnapshots = new Map<string, Dashboard>();
  private feedbackReceipts = new Map<string, string>();

  private snapshot() {
    return DashboardSchema.parse(structuredClone(this.state));
  }
  private pruneTrends() {
    const visible = new Set(this.state.items.map((item) => item.id));
    this.state.trends = this.state.trends
      .map((trend) => ({
        ...trend,
        evidenceIds: trend.evidenceIds.filter((id) => visible.has(id)),
      }))
      .filter((trend) => trend.evidenceIds.length >= 2);
  }
  async getDashboard() {
    return this.snapshot();
  }
  async runDigest() {
    this.state.edition.generatedAt = new Date().toISOString();
    this.state.activity = [
      {
        id: crypto.randomUUID(),
        kind: 'digest',
        status: 'complete' as const,
        message: `Edition refreshed with ${this.state.items.length} useful items`,
        occurredAt: new Date().toISOString(),
      },
      ...this.state.activity,
    ].slice(0, 20);
    return this.snapshot();
  }
  async syncSources() {
    this.state.edition.generatedAt = new Date().toISOString();
    this.state.activity = [
      {
        id: crypto.randomUUID(),
        kind: 'sync',
        status: 'complete' as const,
        message: 'Browser demonstration sources refreshed in memory',
        occurredAt: new Date().toISOString(),
      },
      ...this.state.activity,
    ].slice(0, 20);
    return {
      dashboard: this.snapshot(),
      outcome: {
        mode: 'manual_override' as const,
        finality: 'complete' as const,
        changedSources: this.state.sources.length,
        unchangedSources: 0,
        failedSources: 0,
        changedItems: this.state.items.length,
        sourceLimitReached: false,
      },
    };
  }
  async recordFeedback(requestId: string, itemId: string, signal: FeedbackSignal) {
    const payload = `${itemId}:${signal}`;
    const existing = this.feedbackReceipts.get(requestId);
    if (existing !== undefined) {
      if (existing !== payload)
        throw new Error('That request identifier was already used for different feedback.');
      return this.snapshot();
    }
    this.feedbackReceipts.set(requestId, payload);
    this.feedbackSnapshots.set(requestId, this.snapshot());
    this.state.settings.feedbackCount += 1;
    if (signal === 'not_relevant') {
      this.state.items = this.state.items.filter((item) => item.id !== itemId);
      this.state.privacyEpoch += 1;
    }
    if (signal === 'mute_source') {
      this.state.privacyEpoch += 1;
      const sourceId = this.state.items.find((item) => item.id === itemId)?.sourceId;
      this.state.items = this.state.items.filter((item) => item.sourceId !== sourceId);
    }
    this.pruneTrends();
    return this.snapshot();
  }
  async undoFeedback(requestId: string) {
    const previous = this.feedbackSnapshots.get(requestId);
    if (previous) {
      const privacyEpoch = this.state.privacyEpoch;
      this.state = structuredClone(previous);
      this.state.privacyEpoch = Math.max(privacyEpoch, previous.privacyEpoch);
    }
    return this.snapshot();
  }
  async updateSettings(_requestId: string, settings: Settings) {
    const candidate = SettingsSchema.parse(settings);
    this.state.settings = candidate;
    return this.snapshot();
  }
  async addRssSource(_requestId: string, label: string, url: string) {
    const parsed = new URL(url);
    if (!['http:', 'https:'].includes(parsed.protocol)) throw new Error('Unsafe feed URL');
    const id = `rss-${crypto.randomUUID()}`;
    this.state.sources.push({
      id,
      kind: 'rss',
      label,
      detail: `RSS · ${parsed.hostname}`,
      status: 'healthy',
      healthDetail: 'Demo RSS source is ready.',
      commentsStatus: 'unavailable',
      commentsTruncated: false,
      syncFinality: 'complete',
      lastSync: new Date().toISOString(),
      nextSync: null,
      itemCount: 0,
    });
    return this.snapshot();
  }
  async importArchive(): Promise<ArchiveImportResult> {
    return {
      status: 'canceled',
      sourceId: null,
      importedItems: 0,
      skippedItems: 0,
      changedItems: 0,
      dashboard: this.snapshot(),
    };
  }
  async openOriginal(): Promise<void> {
    throw new Error('Original links open only in the native desktop app.');
  }
  async deleteSource(_requestId: string, sourceId: string) {
    this.state.privacyEpoch += 1;
    this.state.sources = this.state.sources.filter((source) => source.id !== sourceId);
    this.state.items = this.state.items.filter((item) => item.sourceId !== sourceId);
    this.pruneTrends();
    return this.snapshot();
  }
  async resetLearning() {
    const connected = new Set(this.state.sources.map((source) => source.id));
    this.state.items = structuredClone(demoDashboard.items).filter((item) =>
      connected.has(item.sourceId),
    );
    this.state.settings.feedbackCount = 0;
    this.state.trends = structuredClone(demoDashboard.trends).filter((trend) =>
      trend.evidenceIds.every((id) => this.state.items.some((item) => item.id === id)),
    );
    // Completed request receipts survive reset, so a delayed retry cannot restore feedback.
    this.feedbackSnapshots.clear();
    return this.snapshot();
  }
}

export let transport: AppTransport = isTauri() ? new TauriTransport() : new DemoTransport();

export const setTransportForTests = (next: AppTransport) => {
  transport = next;
};

export const createDemoTransport = (): AppTransport => new DemoTransport();
