interface SlashCommand {
  cmd: string;
  usage: string;
  desc: string;
}

interface SlashCommandPaletteProps {
  commands: SlashCommand[];
  selectedIndex: number;
  onSelect: (cmd: string) => void;
}

export default function SlashCommandPalette({ commands, selectedIndex, onSelect }: SlashCommandPaletteProps) {
  if (commands.length === 0) return null;
  return (
    <div role="listbox" aria-label="Command suggestions" className="cmd-palette">
      {commands.map((c, i) => (
        <div
          key={c.cmd}
          role="option"
          aria-selected={i === selectedIndex}
          className={`cmd-palette__item${i === selectedIndex ? ' cmd-palette__item--selected' : ''}`}
          onMouseDown={(e) => {
            e.preventDefault();
            onSelect(c.cmd);
          }}
        >
          <span className="cmd-palette__cmd">{c.cmd}</span>
          <span className="cmd-palette__desc">{c.desc}</span>
        </div>
      ))}
    </div>
  );
}
