import { invoke } from '@tauri-apps/api/core';
import { z } from 'zod';
import { demoDashboard } from './demoData';
import {
  ArchiveImportResultSchema,
  DashboardSchema,
  EditionDetailSchema,
  MastodonProbeResultSchema,
  SettingsSchema,
  SyncOutcomeSchema,
  type ArchiveImportPlatform,
  type ArchiveImportResult,
  LibraryItemSchema,
  OpmlCandidateSchema,
  type LibraryItem,
  type MastodonProbeResult,
  type OpmlCandidate,
  type Dashboard,
  type EditionDetail,
  type FeedbackSignal,
  type Settings,
  type SyncSourcesResult,
} from './types';

export interface AppTransport {
  getDashboard(): Promise<Dashboard>;
  getEdition(editionId: string): Promise<EditionDetail>;
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
  searchLibrary(query: string): Promise<LibraryItem[]>;
  savedLibrary(): Promise<LibraryItem[]>;
  setSaved(requestId: string, postId: string, saved: boolean): Promise<Dashboard>;
  renameSource(requestId: string, sourceId: string, label: string): Promise<Dashboard>;
  setSourcePaused(requestId: string, sourceId: string, paused: boolean): Promise<Dashboard>;
  syncSource(requestId: string, sourceId: string): Promise<Dashboard>;
  pickOpml(): Promise<OpmlCandidate[]>;
  discoverFeeds(url: string): Promise<OpmlCandidate[]>;
  probeMastodonInstance(instanceUrl: string): Promise<MastodonProbeResult>;
  connectMastodon(requestId: string, label: string, instanceUrl: string): Promise<Dashboard>;
  exportOpml(): Promise<boolean>;
  exportBackup(): Promise<boolean>;
  exportSavedItems(): Promise<boolean>;
  restoreBackup(requestId: string): Promise<Dashboard>;
  installedModels(): Promise<string[]>;
}

export const isTauri = () => '__TAURI_INTERNALS__' in window;

export const parseDashboard = (value: unknown) => DashboardSchema.parse(value);

class TauriTransport implements AppTransport {
  async getDashboard() {
    return parseDashboard(await invoke('get_dashboard'));
  }
  async getEdition(editionId: string) {
    return EditionDetailSchema.parse(await invoke('get_edition', { editionId }));
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
  async searchLibrary(query: string) {
    return LibraryItemSchema.array().parse(await invoke('search_library', { request: { query } }));
  }
  async savedLibrary() {
    return LibraryItemSchema.array().parse(await invoke('saved_library'));
  }
  async setSaved(requestId: string, postId: string, saved: boolean) {
    return parseDashboard(await invoke('set_saved', { request: { requestId, postId, saved } }));
  }
  async renameSource(requestId: string, sourceId: string, label: string) {
    return parseDashboard(
      await invoke('rename_source', { request: { requestId, sourceId, label } }),
    );
  }
  async setSourcePaused(requestId: string, sourceId: string, paused: boolean) {
    return parseDashboard(
      await invoke('set_source_paused', { request: { requestId, sourceId, paused } }),
    );
  }
  async syncSource(requestId: string, sourceId: string) {
    return parseDashboard(await invoke('sync_source', { request: { requestId, sourceId } }));
  }
  async pickOpml() {
    return OpmlCandidateSchema.array().parse(await invoke('pick_opml'));
  }
  async discoverFeeds(url: string) {
    return OpmlCandidateSchema.array().parse(await invoke('discover_feeds', { request: { url } }));
  }
  async probeMastodonInstance(instanceUrl: string) {
    return MastodonProbeResultSchema.parse(
      await invoke('probe_mastodon_instance', { request: { instanceUrl } }),
    );
  }
  async connectMastodon(requestId: string, label: string, instanceUrl: string) {
    return parseDashboard(
      await invoke('connect_mastodon', { request: { requestId, label, instanceUrl } }),
    );
  }
  async exportOpml() {
    return Boolean(await invoke('export_opml'));
  }
  async exportBackup() {
    return Boolean(await invoke('export_backup'));
  }
  async exportSavedItems() {
    return Boolean(await invoke('export_saved_items'));
  }
  async restoreBackup(requestId: string) {
    return parseDashboard(await invoke('restore_backup', { request: { requestId } }));
  }
  async installedModels() {
    const value = await invoke('installed_models');
    return z.string().max(200).array().parse(value);
  }
}

class DemoTransport implements AppTransport {
  private state = structuredClone(demoDashboard);
  private feedbackSnapshots = new Map<string, Dashboard>();
  private feedbackSavedSnapshots = new Map<string, Set<string>>();
  private feedbackReceipts = new Map<string, string>();
  private savedPostIds = new Set<string>();

  private snapshot() {
    const snapshot = structuredClone(this.state);
    snapshot.library.savedCount = this.savedPostIds.size;
    return DashboardSchema.parse(snapshot);
  }
  private pruneSaved() {
    const visible = new Set(this.state.items.map((item) => item.id));
    for (const itemId of this.savedPostIds) {
      if (!visible.has(itemId)) this.savedPostIds.delete(itemId);
    }
  }
  private libraryItem(item: (typeof this.state.items)[number]): LibraryItem {
    const source = this.state.sources.find((candidate) => candidate.id === item.sourceId);
    return {
      id: item.id,
      sourceId: item.sourceId,
      source: item.source,
      author: item.author,
      title: item.title,
      excerpt: item.summary,
      publishedAt: item.publishedAt,
      canonicalUrl: item.evidence[0]?.canonicalUrl ?? null,
      saved: this.savedPostIds.has(item.id),
      sourceStatus: source?.status ?? 'unknown',
      sourceHealthDetail: source?.healthDetail ?? 'This source is no longer connected.',
      summaryMethod: item.summaryMethod,
      summaryProvider: item.summaryProvider,
      summaryUncertainty: item.summaryUncertainty,
    };
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
  async getEdition(editionId: string): Promise<EditionDetail> {
    if (editionId !== this.state.edition.id)
      throw new Error('That edition is not available in browser preview.');
    return { edition: this.state.edition, items: this.state.items, trends: this.state.trends };
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
    this.feedbackSavedSnapshots.set(requestId, new Set(this.savedPostIds));
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
    this.pruneSaved();
    this.pruneTrends();
    return this.snapshot();
  }
  async undoFeedback(requestId: string) {
    const previous = this.feedbackSnapshots.get(requestId);
    if (previous) {
      const privacyEpoch = this.state.privacyEpoch;
      this.state = structuredClone(previous);
      this.state.privacyEpoch = Math.max(privacyEpoch, previous.privacyEpoch);
      this.savedPostIds = new Set(this.feedbackSavedSnapshots.get(requestId));
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
    this.pruneSaved();
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
    this.pruneSaved();
    // Completed request receipts survive reset, so a delayed retry cannot restore feedback.
    this.feedbackSnapshots.clear();
    return this.snapshot();
  }
  async searchLibrary(query: string) {
    const needle = query.trim().toLocaleLowerCase();
    if (!needle) return [];
    return this.state.items
      .filter((item) =>
        `${item.title} ${item.summary} ${item.source} ${item.author}`
          .toLocaleLowerCase()
          .includes(needle),
      )
      .map((item) => this.libraryItem(item));
  }
  async savedLibrary() {
    return this.state.items
      .filter((item) => this.savedPostIds.has(item.id))
      .map((item) => this.libraryItem(item));
  }
  async setSaved(_requestId: string, postId: string, saved: boolean) {
    if (!this.state.items.some((item) => item.id === postId)) throw new Error('Item not found');
    if (saved) this.savedPostIds.add(postId);
    else this.savedPostIds.delete(postId);
    return this.snapshot();
  }
  async renameSource(_requestId: string, sourceId: string, label: string) {
    const source = this.state.sources.find((candidate) => candidate.id === sourceId);
    if (!source) throw new Error('Source not found');
    source.label = label;
    return this.snapshot();
  }
  async setSourcePaused(_requestId: string, sourceId: string, paused: boolean) {
    const source = this.state.sources.find((candidate) => candidate.id === sourceId);
    if (!source || source.kind !== 'rss') throw new Error('Only RSS sources can be paused');
    source.status = paused ? 'paused' : 'healthy';
    return this.snapshot();
  }
  async syncSource(_requestId: string, sourceId: string) {
    if (!this.state.sources.some((source) => source.id === sourceId))
      throw new Error('Source not found');
    return this.snapshot();
  }
  async pickOpml() {
    return [];
  }
  async discoverFeeds(url: string) {
    const parsed = new URL(url);
    return [{ label: parsed.hostname, url: parsed.toString() }];
  }
  async probeMastodonInstance(_instanceUrl: string): Promise<MastodonProbeResult> {
    void _instanceUrl;
    throw new Error('Mastodon compatibility checks are available only in the native desktop app.');
  }
  async connectMastodon(
    _requestId: string,
    _label: string,
    _instanceUrl: string,
  ): Promise<Dashboard> {
    void _requestId;
    void _label;
    void _instanceUrl;
    throw new Error('Mastodon connection is available only in the native desktop app.');
  }
  async exportOpml() {
    return false;
  }
  async exportBackup() {
    return false;
  }
  async exportSavedItems() {
    return false;
  }
  async restoreBackup(requestId: string): Promise<Dashboard> {
    void requestId;
    throw new Error('Backup restore is available in the native desktop app.');
  }
  async installedModels() {
    return [];
  }
}

export let transport: AppTransport = isTauri() ? new TauriTransport() : new DemoTransport();

export const setTransportForTests = (next: AppTransport) => {
  transport = next;
};

export const createDemoTransport = (): AppTransport => new DemoTransport();
