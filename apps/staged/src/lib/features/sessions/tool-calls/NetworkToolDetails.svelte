<script lang="ts">
  import type { RichToolItem } from '../acpTranscript';
  import { formatJson } from '../acpTranscript';
  import { isRecord, type ToolCallViewModel } from '../toolCallViewModel';
  import OutputSections from './OutputSections.svelte';

  interface NetworkInfo {
    method: string | null;
    url: string | null;
    status: string | null;
    statusTone: 'normal' | 'success' | 'danger';
    title: string | null;
    selector: string | null;
    screenshot: string | null;
    requestHeaders: string;
    requestBody: string;
    responseHeaders: string;
    responseBody: string;
  }

  interface Props {
    item: RichToolItem;
    viewModel: ToolCallViewModel;
  }

  let { item, viewModel }: Props = $props();
  let network = $derived(extractNetworkInfo(viewModel, item));
  let hasStructuredResponse = $derived(
    !!(
      network.status ||
      network.title ||
      network.screenshot ||
      network.responseHeaders ||
      network.responseBody
    )
  );
  let hasNetworkPreview = $derived(
    !!(
      network.method ||
      network.url ||
      network.status ||
      network.title ||
      network.selector ||
      network.screenshot ||
      network.requestHeaders ||
      network.requestBody ||
      network.responseHeaders ||
      network.responseBody
    )
  );

  function extractNetworkInfo(model: ToolCallViewModel, toolItem: RichToolItem): NetworkInfo {
    const input = model.metadata.input;
    const rawOutput = isRecord(toolItem.rawOutput) ? toolItem.rawOutput : null;
    const request = recordProp(input, 'request');
    const response = recordProp(rawOutput, 'response');
    const statusValue =
      firstValue(rawOutput, ['status', 'statusCode', 'status_code']) ??
      firstValue(response, ['status', 'statusCode', 'status_code']);
    const status = statusValue === undefined || statusValue === null ? null : String(statusValue);

    return {
      method:
        model.metadata.method ??
        normalizeMethod(firstString(request, ['method', 'httpMethod', 'http_method'])),
      url: model.metadata.url ?? firstString(request, ['url', 'uri', 'href']),
      status,
      statusTone: statusTone(statusValue),
      title: firstString(rawOutput, ['title']) ?? firstString(response, ['title']),
      selector: model.metadata.query,
      screenshot: firstString(rawOutput, ['screenshot']) ?? firstString(response, ['screenshot']),
      requestHeaders: valueText(firstValue(input, ['headers']) ?? firstValue(request, ['headers'])),
      requestBody: valueText(firstValue(input, ['body']) ?? firstValue(request, ['body'])),
      responseHeaders: valueText(
        firstValue(rawOutput, ['headers']) ?? firstValue(response, ['headers'])
      ),
      responseBody: valueText(
        firstValue(rawOutput, ['body', 'text', 'content']) ??
          firstValue(response, ['body', 'text', 'content'])
      ),
    };
  }

  function recordProp(
    value: Record<string, unknown> | null,
    key: string
  ): Record<string, unknown> | null {
    if (!value) return null;
    const prop = value[key];
    return isRecord(prop) ? prop : null;
  }

  function firstString(input: Record<string, unknown> | null, keys: string[]): string | null {
    if (!input) return null;
    for (const key of keys) {
      const value = input[key];
      if (typeof value === 'string' && value.trim()) return value;
    }
    return null;
  }

  function firstValue(input: Record<string, unknown> | null, keys: string[]): unknown {
    if (!input) return undefined;
    for (const key of keys) {
      const value = input[key];
      if (value !== undefined && value !== null) return value;
    }
    return undefined;
  }

  function normalizeMethod(value: string | null): string | null {
    return value ? value.toUpperCase() : null;
  }

  function valueText(value: unknown): string {
    if (value === undefined || value === null) return '';
    return typeof value === 'string' ? value : formatJson(value);
  }

  function statusTone(value: unknown): NetworkInfo['statusTone'] {
    const status = typeof value === 'number' ? value : Number(value);
    if (!Number.isFinite(status)) return 'normal';
    if (status >= 200 && status < 400) return 'success';
    if (status >= 400) return 'danger';
    return 'normal';
  }
</script>

<div class="tool-detail-stack">
  {#if hasNetworkPreview}
    <div class="tool-command-panel">
      {#if network.method || network.url || network.status}
        <div class="tool-network-line">
          {#if network.method}
            <span class="tool-status-badge">{network.method}</span>
          {/if}
          {#if network.url}
            <span class="tool-network-url">{network.url}</span>
          {/if}
          {#if network.status}
            <span
              class="tool-status-badge"
              class:success={network.statusTone === 'success'}
              class:danger={network.statusTone === 'danger'}>{network.status}</span
            >
          {/if}
        </div>
      {/if}
      <div class="tool-field-list">
        {#if network.title}
          <span class="tool-field-label">Title</span>
          <span class="tool-field-value">{network.title}</span>
        {/if}
        {#if network.selector}
          <span class="tool-field-label">Selector</span>
          <span class="tool-field-value">{network.selector}</span>
        {/if}
        {#if network.screenshot}
          <span class="tool-field-label">Screenshot</span>
          <span class="tool-field-value">{network.screenshot}</span>
        {/if}
      </div>
    </div>

    {#if network.requestHeaders}
      <section>
        <div class="tool-panel-label">Request headers</div>
        <pre class="tool-code-output">{network.requestHeaders}</pre>
      </section>
    {/if}
    {#if network.requestBody}
      <section>
        <div class="tool-panel-label">Request body</div>
        <pre class="tool-code-output">{network.requestBody}</pre>
      </section>
    {/if}
    {#if network.responseHeaders}
      <section>
        <div class="tool-panel-label">Response headers</div>
        <pre class="tool-code-output">{network.responseHeaders}</pre>
      </section>
    {/if}
    {#if network.responseBody}
      <section>
        <div class="tool-panel-label">Response body</div>
        <pre class="tool-code-output">{network.responseBody}</pre>
      </section>
    {/if}
  {/if}

  <OutputSections
    {viewModel}
    includePrimary={!network.responseBody}
    includeRaw={!hasStructuredResponse}
  />
</div>
