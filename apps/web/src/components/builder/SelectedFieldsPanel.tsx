/**
 * SelectedFieldsPanel — displays and manages the list of fields the user
 * has clicked-to-select from the Document Renderer.
 *
 * Reads from and writes to builderStore.mappingSlice directly.
 * Drag-to-reorder via HTML5 DnD + keyboard (Enter/Space + Arrow Up/Down).
 * Touch devices get 'Move up'/'Move down' buttons instead.
 */

import { useRef, useMemo, useState } from 'react';
import { useBuilderStore } from '@stores/builderStore';
import type { SelectedField } from '@stores/builderStore';
import { useDragToReorder } from '@hooks/useDragToReorder';
import { filterFieldNameInput } from '@/lib/fieldMappingUtils';

// ── Type badge colors ──────────────────────────────────────────────────────────

const TYPE_BADGE_CLASSES: Record<string, string> = {
  string: 'bg-surface-variant text-text-secondary',
  number: 'bg-blue-100 text-blue-700 dark:bg-blue-900 dark:text-blue-300',
  boolean: 'bg-secondary-container text-secondary',
  date: 'bg-purple-100 text-purple-700 dark:bg-purple-900 dark:text-purple-300',
  array: 'bg-orange-100 text-orange-700 dark:bg-orange-900 dark:text-orange-300',
  object: 'bg-surface-container-highest text-text-secondary',
};

function typeBadgeClass(type: string): string {
  return TYPE_BADGE_CLASSES[type] ?? TYPE_BADGE_CLASSES['string'] ?? 'bg-surface-variant text-text-secondary';
}

// ── GripIcon ──────────────────────────────────────────────────────────────────

function GripIcon() {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="currentColor"
      className="w-14 h-14"
    >
      <circle cx="5" cy="4" r="1.2" />
      <circle cx="11" cy="4" r="1.2" />
      <circle cx="5" cy="8" r="1.2" />
      <circle cx="11" cy="8" r="1.2" />
      <circle cx="5" cy="12" r="1.2" />
      <circle cx="11" cy="12" r="1.2" />
    </svg>
  );
}

// ── FieldRow ──────────────────────────────────────────────────────────────────

interface FieldRowProps {
  field: SelectedField;
  index: number;
  total: number;
  isDuplicate: boolean;
  isKeyboardDragging: boolean;
  isDropTarget: boolean;
  isBeingDragged: boolean;
  isTouch: boolean;
  itemDragProps: ReturnType<ReturnType<typeof useDragToReorder>['getItemProps']>;
  dragHandleProps: ReturnType<ReturnType<typeof useDragToReorder>['getDragHandleProps']>;
  onMoveUp: () => void;
  onMoveDown: () => void;
  onNameChange: (jsonPath: string, value: string) => void;
  onNameBlur: (jsonPath: string, currentValue: string) => void;
  onRemove: (jsonPath: string) => void;
}

function FieldRow({
  field,
  index,
  total,
  isDuplicate,
  isKeyboardDragging,
  isDropTarget,
  isBeingDragged,
  isTouch,
  itemDragProps,
  dragHandleProps,
  onMoveUp,
  onMoveDown,
  onNameChange,
  onNameBlur,
  onRemove,
}: FieldRowProps) {
  const badgeClass = typeBadgeClass(field.type);

  const rowClasses = [
    'flex items-start gap-8 rounded-md border p-10 transition-colors',
    isBeingDragged ? 'opacity-40' : 'opacity-100',
    isDropTarget ? 'border-brand-400 bg-brand-50 dark:bg-brand-950' : 'border-border-subtle bg-surface-container-lowest',
    isKeyboardDragging ? 'ring-2 ring-brand-500 border-brand-500' : '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div {...itemDragProps} className={rowClasses} data-field-json-path={field.jsonPath}>
      {/* Drag handle (mouse) or move buttons (touch) */}
      {isTouch ? (
        <div className="flex flex-col gap-2 shrink-0 pt-2">
          <button
            type="button"
            onClick={onMoveUp}
            disabled={index === 0}
            aria-label={`Move field ${index + 1} up`}
            className="text-xs text-text-secondary hover:text-text-primary disabled:opacity-30 focus:outline-none focus:ring-1 focus:ring-brand-500 rounded"
          >
            ▲
          </button>
          <button
            type="button"
            onClick={onMoveDown}
            disabled={index === total - 1}
            aria-label={`Move field ${index + 1} down`}
            className="text-xs text-text-secondary hover:text-text-primary disabled:opacity-30 focus:outline-none focus:ring-1 focus:ring-brand-500 rounded"
          >
            ▼
          </button>
        </div>
      ) : (
        <span
          {...dragHandleProps}
          className="shrink-0 pt-2 text-text-secondary hover:text-text-primary cursor-grab active:cursor-grabbing focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
        >
          <GripIcon />
        </span>
      )}

      {/* Main content */}
      <div className="flex-1 min-w-0 space-y-6">
        {/* Name input */}
        <div>
          <input
            type="text"
            data-testid="field-name-input"
            value={field.name}
            onChange={(e) => onNameChange(field.jsonPath, e.target.value)}
            onBlur={(e) => onNameBlur(field.jsonPath, e.target.value)}
            aria-label={`Field name for ${field.jsonPath}`}
            aria-invalid={isDuplicate}
            aria-describedby={isDuplicate ? `dup-err-${field.jsonPath}` : undefined}
            className={[
              'w-full rounded border px-8 py-4 text-sm font-medium text-text-primary bg-transparent',
              'focus:outline-none focus:ring-2 focus:ring-brand-500',
              isDuplicate ? 'ring-2 ring-error-DEFAULT border-error-DEFAULT' : 'border-border-subtle',
            ].join(' ')}
          />
          {isDuplicate && (
            <p
              id={`dup-err-${field.jsonPath}`}
              role="alert"
              className="mt-4 text-xs text-error-DEFAULT"
            >
              A field named {field.name} already exists
            </p>
          )}
        </div>

        {/* Type badge + example value */}
        <div className="flex items-center gap-8 flex-wrap min-w-0">
          <span
            data-testid="field-type-badge"
            className={`shrink-0 inline-flex items-center rounded-full px-8 py-2 text-xs font-medium ${badgeClass}`}
          >
            {field.type}
          </span>
          {field.example.length > 0 && (
            <span
              data-testid="field-example-value"
              title={field.example}
              className="text-xs text-text-secondary truncate min-w-0"
            >
              {field.example}
            </span>
          )}
        </div>
      </div>

      {/* Remove button */}
      <button
        type="button"
        data-testid="field-remove-btn"
        onClick={() => onRemove(field.jsonPath)}
        aria-label={`Remove field ${field.name}`}
        className="shrink-0 mt-2 rounded text-text-secondary hover:text-error-DEFAULT focus:outline-none focus:ring-2 focus:ring-brand-500 transition-colors"
      >
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="currentColor" className="w-16 h-16">
          <path
            fillRule="evenodd"
            d="M4.293 4.293a1 1 0 011.414 0L8 6.586l2.293-2.293a1 1 0 111.414 1.414L9.414 8l2.293 2.293a1 1 0 01-1.414 1.414L8 9.414l-2.293 2.293a1 1 0 01-1.414-1.414L6.586 8 4.293 5.707a1 1 0 010-1.414z"
            clipRule="evenodd"
          />
        </svg>
      </button>
    </div>
  );
}

// ── SelectedFieldsPanel ───────────────────────────────────────────────────────

/**
 * Panel showing all fields the user has selected from the Document Renderer.
 * Supports drag-to-reorder (mouse + keyboard + touch), field name editing,
 * duplicate detection, and bulk clear with confirmation.
 */
export function SelectedFieldsPanel() {
  const selectedFields = useBuilderStore((s) => s.mappingSlice.selectedFields);
  const reorderSelectedFields = useBuilderStore((s) => s.reorderSelectedFields);
  const clearSelectedFields = useBuilderStore((s) => s.clearSelectedFields);
  const removeSelectedField = useBuilderStore((s) => s.removeSelectedField);
  const updateFieldName = useBuilderStore((s) => s.updateFieldName);

  // Track last valid (non-empty) name for each field to revert on blur.
  const lastValidNamesRef = useRef<Map<string, string>>(new Map());

  // Inline clear-all confirmation state.
  const [showClearConfirm, setShowClearConfirm] = useState(false);

  const {
    getItemProps,
    getDragHandleProps,
    moveItem,
    announcement,
    isTouch,
    keyboardDragIndex,
    isDraggingIndex,
    dropTargetIndex,
  } = useDragToReorder({
    items: selectedFields,
    onReorder: reorderSelectedFields,
  });

  // Detect duplicate field names (case-insensitive).
  const nameCountMap = useMemo(() => {
    const counts = new Map<string, number>();
    for (const f of selectedFields) {
      const key = f.name.toLowerCase();
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [selectedFields]);

  const isDuplicate = (name: string) =>
    name.length > 0 && (nameCountMap.get(name.toLowerCase()) ?? 0) > 1;

  const handleNameChange = (jsonPath: string, rawValue: string) => {
    const filtered = filterFieldNameInput(rawValue);
    updateFieldName(jsonPath, filtered);
    if (filtered.length > 0) {
      lastValidNamesRef.current.set(jsonPath, filtered);
    }
  };

  const handleNameBlur = (jsonPath: string, currentValue: string) => {
    if (currentValue.length === 0) {
      const lastValid = lastValidNamesRef.current.get(jsonPath) ?? 'field';
      updateFieldName(jsonPath, lastValid);
    }
  };

  const handleClearAll = () => {
    clearSelectedFields();
    setShowClearConfirm(false);
  };

  const fieldCount = selectedFields.length;

  return (
    <section
      data-testid="selected-fields-panel"
      className="flex flex-col gap-16"
      aria-label="Selected fields"
    >
      {/* Header */}
      <div className="flex items-center justify-between gap-8">
        <h3
          data-testid="field-count"
          className="text-sm font-medium text-text-primary"
        >
          {fieldCount === 1 ? '1 field selected' : `${fieldCount} fields selected`}
        </h3>

        {fieldCount >= 2 && !showClearConfirm && (
          <button
            type="button"
            data-testid="clear-all-fields-btn"
            onClick={() => setShowClearConfirm(true)}
            className="text-xs text-text-secondary hover:text-error-DEFAULT focus:outline-none focus:ring-2 focus:ring-brand-500 rounded transition-colors"
          >
            Clear all
          </button>
        )}

        {showClearConfirm && (
          <div className="flex items-center gap-8">
            <span className="text-xs text-text-secondary">Remove all fields?</span>
            <button
              type="button"
              onClick={handleClearAll}
              className="text-xs font-medium text-error-DEFAULT hover:underline focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
            >
              Clear all
            </button>
            <button
              type="button"
              onClick={() => setShowClearConfirm(false)}
              className="text-xs text-text-secondary hover:text-text-primary focus:outline-none focus:ring-2 focus:ring-brand-500 rounded"
            >
              Cancel
            </button>
          </div>
        )}
      </div>

      {/* Empty state */}
      {fieldCount === 0 && (
        <div
          data-testid="fields-empty-state"
          className="rounded-md border border-dashed border-border-subtle bg-surface-container-low py-32 px-16 text-center"
        >
          <p className="text-sm text-text-secondary">
            Click any value in the response to add it
          </p>
        </div>
      )}

      {/* aria-live region for drag announcements */}
      <div aria-live="polite" aria-atomic="true" className="sr-only">
        {announcement}
      </div>

      {/* Field list */}
      {fieldCount > 0 && (
        <div className="space-y-8">
          {selectedFields.map((field, index) => (
            <FieldRow
              key={field.jsonPath}
              field={field}
              index={index}
              total={fieldCount}
              isDuplicate={isDuplicate(field.name)}
              isKeyboardDragging={keyboardDragIndex === index}
              isDropTarget={dropTargetIndex === index && isDraggingIndex !== index}
              isBeingDragged={isDraggingIndex === index}
              isTouch={isTouch}
              itemDragProps={getItemProps(index)}
              dragHandleProps={getDragHandleProps(index)}
              onMoveUp={() => moveItem(index, index - 1)}
              onMoveDown={() => moveItem(index, index + 1)}
              onNameChange={handleNameChange}
              onNameBlur={handleNameBlur}
              onRemove={removeSelectedField}
            />
          ))}
        </div>
      )}
    </section>
  );
}
