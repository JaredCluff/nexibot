import React from 'react';
import ReactDOM from 'react-dom/client';
import { getCurrentWindow } from '@tauri-apps/api/window';
import App from './App';
import ShellViewerApp from './components/ShellViewerApp';
import './styles.css';

// Detect window label synchronously at startup.
// In Tauri v2, Window.label is a string property (not async).
// Falls back to 'main' outside a Tauri context (tests, browser).
function detectWindowLabel(): string {
  try {
    return getCurrentWindow().label;
  } catch {
    return 'main';
  }
}

const windowLabel = detectWindowLabel();
const Root = windowLabel === 'shell-viewer' ? ShellViewerApp : App;

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
