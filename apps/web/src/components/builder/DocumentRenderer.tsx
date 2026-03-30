/**
 * DocumentRenderer — renders an API response as a structured, human-readable
 * document with clickable values for field mapping.
 *
 * Purely presentational: no Zustand access, no network calls, no side effects.
 * All interaction is handled via the `onFieldSelect` callback.
 */

import React, { useState, useMemo } from 'react';
import { formatFieldName, formatValue, buildJsonPath } from '@/lib/rendererFormatters';

// ── Constants ─────────────────────────────────────────────────────────────────

const MAX_TABLE_ROWS = 5;
const MAX_TABLE_COLS = 8;
/** Number of columns always visible on mobile (<md breakpoint). */
const MOBILE_TABLE_COLS = 3;

// ── Public types ──────────────────────────────────────────────────────────────

export interface DocumentRendererProps {
  data: unknown;
  selectedPaths: Set<string>;
  onFieldSelect: (jsonPath: string, value: unknown) => void;
}

// ── RendererErrorBoundary ─────────────────────────────────────────────────────

interface RendererErrorBoundaryProps {
  children: React.ReactNode;
  rawData?: unknown;
}

interface RendererErrorBoundaryState {
  hasError: boolean;
  showRaw: boolean;
}

export class RendererErrorBoundary extends React.Component<
  RendererErrorBoundaryProps,
  RendererErrorBoundaryState
> {
  constructor(props: RendererErrorBoundaryProps) {
    super(props);
    this.state = { hasError: false, showRaw: false };
  }

  static getDerivedStateFromError(): Partial<RendererErrorBoundaryState> {
    return { hasError: true };
  }

  render() {
    if (!this.state.hasError) {
      return this.props.children;
    }

    let rawJson = '';
    try {
      rawJson = JSON.stringify(this.props.rawData, null, 2);
    } catch {
      rawJson = '<Could not serialise response data>';
    }

    return (
      <div className="rounded-md border border-border-subtle bg-surface-container p-24 space-y-12">
        <p className="text-sm font-medium text-text-primary">
          Could not render this response
        </p>
        {!this.state.showRaw ? (
          <button
            type="button"
            onClick={() => this.setState({ showRaw: true })}
            className="text-sm text-brand-500 hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
          >
            View raw JSON
          </button>
        ) : (
          <pre className="overflow-x-auto rounded-md bg-surface-container-highest p-16 text-xs font-mono text-text-secondary max-h-[400px] overflow-y-auto">
            {rawJson}
          </pre>
        )}
      </div>
    );
  }
}

// ── Shared props interface ────────────────────────────────────────────────────

interface SharedRenderProps {
  selectedPaths: Set<string>;
  onFieldSelect: (jsonPath: string, value: unknown) => void;
}

// ── ScalarValue ───────────────────────────────────────────────────────────────

interface ScalarValueProps extends SharedRenderProps {
  value: unknown;
  jsonPath: string;
  /** Original object key — used for currency/percent format heuristics. */
  fieldKey?: string | undefined;
}

/**
 * Renders a single scalar value as a clickable element.
 * Highlights selected paths and shows a ring on hover.
 */
function ScalarValue({ value, jsonPath, fieldKey, selectedPaths, onFieldSelect }: ScalarValueProps) {
  const formatted = formatValue(value, fieldKey);
  const isSelected = selectedPaths.has(jsonPath);

  const baseClasses =
    'inline-flex items-center rounded-sm px-4 py-2 text-sm cursor-pointer ' +
    'transition-colors focus:outline-none focus:ring-2 focus:ring-brand-500 focus:ring-offset-1';
  const hoverClasses = 'hover:ring-2 hover:ring-brand-300';
  const selectedClasses = isSelected
    ? 'bg-brand-100 dark:bg-brand-900 ring-2 ring-brand-500'
    : '';

  const handleClick = () => {
    onFieldSelect(jsonPath, value);
  };

  // null / empty
  if (formatted.type === 'null') {
    return (
      <button
        type="button"
        data-testid="renderer-value"
        data-jsonpath={jsonPath}
        onClick={handleClick}
        aria-label="empty"
        className={`${baseClasses} ${hoverClasses} ${selectedClasses} text-text-secondary italic`}
      >
        —
      </button>
    );
  }

  // boolean → colored pill
  if (formatted.type === 'boolean') {
    const isTrue = value === true;
    return (
      <button
        type="button"
        data-testid="renderer-value"
        data-jsonpath={jsonPath}
        onClick={handleClick}
        className={`${baseClasses} ${hoverClasses} ${selectedClasses} text-xs font-semibold px-8 rounded-full ${
          isTrue
            ? 'bg-secondary-container text-secondary'
            : 'bg-surface-variant text-text-secondary'
        }`}
      >
        {formatted.display}
      </button>
    );
  }

  // number, string, date
  return (
    <button
      type="button"
      data-testid="renderer-value"
      data-jsonpath={jsonPath}
      onClick={handleClick}
      className={`${baseClasses} ${hoverClasses} ${selectedClasses} text-text-primary`}
    >
      {formatted.display}
    </button>
  );
}

// ── ScalarList — inline or bulleted list of scalar array items ────────────────

interface ScalarListProps extends SharedRenderProps {
  items: unknown[];
  parentPath: string;
}

function ScalarList({ items, parentPath, selectedPaths, onFieldSelect }: ScalarListProps) {
  // ≤ 5 items: inline comma-separated span
  if (items.length <= 5) {
    return (
      <span className="inline-flex flex-wrap gap-4 items-center">
        {items.map((item, i) => {
          const itemPath = buildJsonPath(i, parentPath);
          return (
            <React.Fragment key={i}>
              <ScalarValue
                value={item}
                jsonPath={itemPath}
                selectedPaths={selectedPaths}
                onFieldSelect={onFieldSelect}
              />
              {i < items.length - 1 && (
                <span className="text-text-secondary text-xs select-none">,</span>
              )}
            </React.Fragment>
          );
        })}
      </span>
    );
  }

  // > 5 items: bulleted list
  return (
    <ul className="list-disc list-inside space-y-4 text-sm">
      {items.map((item, i) => {
        const itemPath = buildJsonPath(i, parentPath);
        return (
          <li key={i}>
            <ScalarValue
              value={item}
              jsonPath={itemPath}
              selectedPaths={selectedPaths}
              onFieldSelect={onFieldSelect}
            />
          </li>
        );
      })}
    </ul>
  );
}

// ── ObjectTable — array-of-objects rendered as a responsive table ─────────────

interface ObjectTableProps extends SharedRenderProps {
  rows: Record<string, unknown>[];
  parentPath: string;
}

function ObjectTable({ rows, parentPath, selectedPaths, onFieldSelect }: ObjectTableProps) {
  const [showAllRows, setShowAllRows] = useState(false);
  const [showAllCols, setShowAllCols] = useState(false);

  // Collect all unique column keys, preserving the first-seen order.
  const allKeys = useMemo(() => {
    const keys: string[] = [];
    const seen = new Set<string>();
    for (const row of rows) {
      for (const k of Object.keys(row)) {
        if (!seen.has(k)) {
          seen.add(k);
          keys.push(k);
        }
      }
    }
    return keys;
  }, [rows]);

  const shownRows = showAllRows ? rows : rows.slice(0, MAX_TABLE_ROWS);
  const shownKeys = showAllCols ? allKeys : allKeys.slice(0, MAX_TABLE_COLS);
  const hiddenRowCount = rows.length - MAX_TABLE_ROWS;
  const hiddenColCount = allKeys.length - MAX_TABLE_COLS;

  const hasFooterActions =
    (hiddenRowCount > 0 && !showAllRows) || (hiddenColCount > 0 && !showAllCols);

  return (
    <div className="overflow-x-auto rounded-md border border-border-subtle">
      <table className="w-full text-sm border-collapse">
        <thead>
          <tr className="border-b border-border-subtle bg-surface-container-low">
            {shownKeys.map((key, colIdx) => (
              <th
                key={key}
                data-field-key={key}
                scope="col"
                className={`text-left px-12 py-8 font-medium text-text-secondary whitespace-nowrap${
                  colIdx >= MOBILE_TABLE_COLS ? ' hidden md:table-cell' : ''
                }`}
              >
                {formatFieldName(key)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {shownRows.map((row, rowIdx) => {
            const rowPath = buildJsonPath(rowIdx, parentPath);
            return (
              <tr
                key={rowIdx}
                className="border-b border-border-subtle last:border-b-0 hover:bg-surface-container-low transition-colors"
              >
                {shownKeys.map((key, colIdx) => {
                  const rawCell = row[key];
                  const cellValue = rawCell !== undefined ? rawCell : null;
                  const cellPath = buildJsonPath(key, rowPath);
                  const isScalar = cellValue === null || typeof cellValue !== 'object';

                  let cellContent: React.ReactNode;
                  if (isScalar) {
                    cellContent = (
                      <ScalarValue
                        value={cellValue}
                        jsonPath={cellPath}
                        fieldKey={key}
                        selectedPaths={selectedPaths}
                        onFieldSelect={onFieldSelect}
                      />
                    );
                  } else {
                    // Non-scalar in table cell: show truncated JSON as fallback.
                    let truncated = '';
                    try {
                      const s = JSON.stringify(cellValue);
                      truncated = s.length > 40 ? s.slice(0, 40) + '…' : s;
                    } catch {
                      truncated = '[complex]';
                    }
                    cellContent = (
                      <span className="text-text-secondary text-xs font-mono">{truncated}</span>
                    );
                  }

                  return (
                    <td
                      key={key}
                      className={`px-12 py-8${colIdx >= MOBILE_TABLE_COLS ? ' hidden md:table-cell' : ''}`}
                    >
                      {cellContent}
                    </td>
                  );
                })}
              </tr>
            );
          })}
        </tbody>
      </table>

      {hasFooterActions && (
        <div className="flex gap-16 px-12 py-8 border-t border-border-subtle bg-surface-container-low">
          {hiddenRowCount > 0 && !showAllRows && (
            <button
              type="button"
              onClick={() => setShowAllRows(true)}
              className="text-sm text-brand-500 hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
            >
              Show {hiddenRowCount} more
            </button>
          )}
          {hiddenColCount > 0 && !showAllCols && (
            <button
              type="button"
              onClick={() => setShowAllCols(true)}
              className="text-sm text-brand-500 hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
            >
              Show more columns
            </button>
          )}
        </div>
      )}
    </div>
  );
}

// ── ArrayNode — dispatches to table or scalar list ────────────────────────────

interface ArrayNodeProps extends SharedRenderProps {
  arr: unknown[];
  parentPath: string;
}

function ArrayNode({ arr, parentPath, selectedPaths, onFieldSelect }: ArrayNodeProps) {
  if (arr.length === 0) {
    return <span className="text-text-secondary italic text-sm">No items</span>;
  }

  // Render as table only when every element is a non-null, non-array object.
  const isObjectArray = arr.every(
    (item) => item !== null && typeof item === 'object' && !Array.isArray(item)
  );

  if (isObjectArray) {
    return (
      <ObjectTable
        rows={arr as Record<string, unknown>[]}
        parentPath={parentPath}
        selectedPaths={selectedPaths}
        onFieldSelect={onFieldSelect}
      />
    );
  }

  return (
    <ScalarList
      items={arr}
      parentPath={parentPath}
      selectedPaths={selectedPaths}
      onFieldSelect={onFieldSelect}
    />
  );
}

// ── RenderNode — recursive dispatcher ─────────────────────────────────────────

interface RenderNodeProps extends SharedRenderProps {
  value: unknown;
  /** JSONPath of the current value. */
  parentPath: string;
  /** 0 = root object/array; increments with each nested container. */
  depth: number;
  /** Original key that produced this value (for format heuristics). */
  fieldKey?: string | undefined;
}

function RenderNode({ value, parentPath, depth, fieldKey, selectedPaths, onFieldSelect }: RenderNodeProps) {
  // ── Scalar ───────────────────────────────────────────────────────────────
  if (value === null || typeof value !== 'object') {
    return (
      <ScalarValue
        value={value}
        jsonPath={parentPath}
        fieldKey={fieldKey}
        selectedPaths={selectedPaths}
        onFieldSelect={onFieldSelect}
      />
    );
  }

  // ── Array ─────────────────────────────────────────────────────────────────
  if (Array.isArray(value)) {
    return (
      <ArrayNode
        arr={value}
        parentPath={parentPath}
        selectedPaths={selectedPaths}
        onFieldSelect={onFieldSelect}
      />
    );
  }

  // ── Object ────────────────────────────────────────────────────────────────
  const obj = value as Record<string, unknown>;
  const entries = Object.entries(obj);

  if (entries.length === 0) {
    return <span className="text-text-secondary italic text-sm">Empty object</span>;
  }

  return (
    <div className="space-y-8">
      {entries.map(([key, val]) => {
        const fieldPath = buildJsonPath(key, parentPath);
        const isScalar = val === null || typeof val !== 'object';

        // ── Scalar field: label + value on same row ──────────────────────
        if (isScalar) {
          return (
            <div key={key} className="flex items-start gap-12 min-h-[28px]">
              <span
                className="text-sm font-medium text-text-secondary shrink-0 w-[140px] truncate pt-2"
                data-field-key={key}
                title={formatFieldName(key)}
              >
                {formatFieldName(key)}
              </span>
              <ScalarValue
                value={val}
                jsonPath={fieldPath}
                fieldKey={key}
                selectedPaths={selectedPaths}
                onFieldSelect={onFieldSelect}
              />
            </div>
          );
        }

        // ── Container field: collapsible details/summary ─────────────────
        const isArr = Array.isArray(val);
        const label = formatFieldName(key);
        const countSuffix = isArr ? ` (${(val as unknown[]).length})` : '';
        // First two depth levels are expanded by default; deeper levels are collapsed.
        const isExpanded = depth < 2;

        return (
          <details key={key} open={isExpanded} className="group">
            <summary
              className="list-none cursor-pointer py-4 flex items-center gap-8 hover:text-primary transition-colors"
              data-field-key={key}
            >
              {/* Disclosure triangle — rotates when open */}
              <span
                aria-hidden="true"
                className="text-text-secondary text-xs transition-transform duration-normal group-open:rotate-90 select-none"
              >
                ▶
              </span>
              <span className="text-sm font-medium text-text-secondary">
                {label}
                {countSuffix}
              </span>
            </summary>
            <div className="ml-20 mt-4 pl-16 border-l-2 border-border-subtle">
              <RenderNode
                value={val}
                parentPath={fieldPath}
                depth={depth + 1}
                selectedPaths={selectedPaths}
                onFieldSelect={onFieldSelect}
              />
            </div>
          </details>
        );
      })}
    </div>
  );
}

// ── DocumentRendererInner ─────────────────────────────────────────────────────

function DocumentRendererInner({ data, selectedPaths, onFieldSelect }: DocumentRendererProps) {
  // ── Pathological inputs ───────────────────────────────────────────────────

  if (data === null || data === undefined) {
    return (
      <div data-testid="document-renderer" className="p-16">
        <p className="text-text-secondary italic text-sm">No data</p>
      </div>
    );
  }

  if (Array.isArray(data) && data.length === 0) {
    return (
      <div data-testid="document-renderer" className="p-16">
        <p className="text-text-secondary italic text-sm">Empty response</p>
      </div>
    );
  }

  // ── Top-level scalar (string, number, boolean) ────────────────────────────
  if (typeof data !== 'object') {
    return (
      <div data-testid="document-renderer" className="p-16 space-y-8">
        <h3 className="text-xs font-medium text-text-secondary uppercase tracking-wider">
          Response
        </h3>
        <ScalarValue
          value={data}
          jsonPath="$"
          selectedPaths={selectedPaths}
          onFieldSelect={onFieldSelect}
        />
      </div>
    );
  }

  // ── Normal object or non-empty array ─────────────────────────────────────
  return (
    <div data-testid="document-renderer" className="p-16">
      <RenderNode
        value={data}
        parentPath="$"
        depth={0}
        selectedPaths={selectedPaths}
        onFieldSelect={onFieldSelect}
      />
    </div>
  );
}

// ── DocumentRenderer (public export) ─────────────────────────────────────────

/**
 * Renders an API response as a structured, clickable document.
 *
 * Wrapped in `<RendererErrorBoundary>` so that deeply nested pathological
 * data can be recovered via the "View raw JSON" fallback.
 */
export function DocumentRenderer(props: DocumentRendererProps) {
  return (
    <RendererErrorBoundary rawData={props.data}>
      <DocumentRendererInner {...props} />
    </RendererErrorBoundary>
  );
}
