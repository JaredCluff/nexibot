// Cross-platform bridge bundling script for Tauri builds
const fs = require('fs');
const path = require('path');

const srcDir = path.resolve(__dirname, '..', 'bridge');
const destDir = path.resolve(__dirname, '_bridge_bundle', 'bridge');

// Remove old bundle
if (fs.existsSync(path.resolve(__dirname, '_bridge_bundle'))) {
  fs.rmSync(path.resolve(__dirname, '_bridge_bundle'), { recursive: true });
}

// Create destination
fs.mkdirSync(destDir, { recursive: true });

// Copy core files
for (const file of ['server.js', 'package.json', 'package-lock.json']) {
  const src = path.join(srcDir, file);
  if (fs.existsSync(src)) {
    fs.copyFileSync(src, path.join(destDir, file));
  }
}

// Copy core node_modules
const srcModules = path.join(srcDir, 'node_modules');
if (fs.existsSync(srcModules)) {
  cpRecursive(srcModules, path.join(destDir, 'node_modules'));
}

// Copy lib/ directory
const srcLib = path.join(srcDir, 'lib');
if (fs.existsSync(srcLib)) {
  cpRecursive(srcLib, path.join(destDir, 'lib'));
}

// Copy each plugin directory (including their node_modules)
const pluginsDir = path.join(srcDir, 'plugins');
if (fs.existsSync(pluginsDir)) {
  for (const entry of fs.readdirSync(pluginsDir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      const pluginSrc = path.join(pluginsDir, entry.name);
      const pluginDest = path.join(destDir, 'plugins', entry.name);
      cpRecursive(pluginSrc, pluginDest);
    }
  }
}

console.log('Bridge bundle created at', destDir);

function cpRecursive(src, dest) {
  fs.mkdirSync(dest, { recursive: true });
  for (const entry of fs.readdirSync(src, { withFileTypes: true })) {
    const srcPath = path.join(src, entry.name);
    const destPath = path.join(dest, entry.name);
    if (entry.isDirectory()) {
      cpRecursive(srcPath, destPath);
    } else {
      fs.copyFileSync(srcPath, destPath);
    }
  }
}
