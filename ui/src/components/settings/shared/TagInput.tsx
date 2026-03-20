import { useState, useRef, useEffect, useMemo } from 'react';
import type { TagSuggestion } from './suggestions';

export type { TagSuggestion };

interface TagInputProps {
  tags: string[];
  onChange: (tags: string[]) => void;
  placeholder?: string;
  suggestions?: TagSuggestion[];
}

export function TagInput({ tags, onChange, placeholder, suggestions }: TagInputProps) {
  const [value, setValue] = useState('');
  const [showDropdown, setShowDropdown] = useState(false);
  const [highlightIndex, setHighlightIndex] = useState(-1);
  const wrapperRef = useRef<HTMLDivElement>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const blurTimeout = useRef<ReturnType<typeof setTimeout>>();

  const filtered = useMemo(() => {
    if (!suggestions) return [];
    const lower = value.toLowerCase();
    return suggestions.filter(
      (s) =>
        !tags.includes(s.value) &&
        (lower === '' || s.value.toLowerCase().includes(lower) || s.description.toLowerCase().includes(lower))
    );
  }, [suggestions, value, tags]);

  useEffect(() => {
    setHighlightIndex(-1);
  }, [filtered.length, value]);

  useEffect(() => {
    if (highlightIndex >= 0 && dropdownRef.current) {
      const item = dropdownRef.current.children[highlightIndex] as HTMLElement | undefined;
      item?.scrollIntoView({ block: 'nearest' });
    }
  }, [highlightIndex]);

  const addTag = (text?: string) => {
    const trimmed = (text ?? value).trim();
    if (trimmed && !tags.includes(trimmed)) {
      onChange([...tags, trimmed]);
      setValue('');
      setShowDropdown(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!showDropdown || filtered.length === 0) {
      if (e.key === 'Enter') {
        e.preventDefault();
        addTag();
      }
      return;
    }

    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        setHighlightIndex((prev) => (prev < filtered.length - 1 ? prev + 1 : 0));
        break;
      case 'ArrowUp':
        e.preventDefault();
        setHighlightIndex((prev) => (prev > 0 ? prev - 1 : filtered.length - 1));
        break;
      case 'Enter':
        e.preventDefault();
        if (highlightIndex >= 0 && highlightIndex < filtered.length) {
          addTag(filtered[highlightIndex].value);
        } else {
          addTag();
        }
        break;
      case 'Escape':
        e.preventDefault();
        setShowDropdown(false);
        break;
    }
  };

  const handleFocus = () => {
    if (suggestions && suggestions.length > 0) {
      setShowDropdown(true);
    }
  };

  const handleBlur = () => {
    blurTimeout.current = setTimeout(() => setShowDropdown(false), 150);
  };

  useEffect(() => {
    return () => {
      if (blurTimeout.current) clearTimeout(blurTimeout.current);
    };
  }, []);

  return (
    <>
      <div className="tag-list">
        {tags.map((tag, i) => (
          <span key={i} className="tag">
            {tag}
            <button onClick={() => onChange(tags.filter((_, j) => j !== i))}>x</button>
          </span>
        ))}
      </div>
      <div className="tag-input-wrapper">
        <div className="tag-input-row">
          <input
            type="text"
            placeholder={placeholder}
            value={value}
            onChange={(e) => {
              setValue(e.target.value);
              if (suggestions && suggestions.length > 0) setShowDropdown(true);
            }}
            onKeyDown={handleKeyDown}
            onFocus={handleFocus}
            onBlur={handleBlur}
          />
          <button onClick={() => addTag()}>Add</button>
        </div>
        {showDropdown && filtered.length > 0 && (
          <div className="tag-suggestions-dropdown" ref={dropdownRef}>
            {filtered.map((s, i) => (
              <div
                key={s.value}
                className={`tag-suggestion-item${i === highlightIndex ? ' highlighted' : ''}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  addTag(s.value);
                }}
                onMouseEnter={() => setHighlightIndex(i)}
              >
                {s.badge && (
                  <span className={`suggestion-badge ${s.badge}`}>{s.badge}</span>
                )}
                <span className="suggestion-value">{s.value}</span>
                <span className="suggestion-desc">— {s.description}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}
