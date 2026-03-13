import fs from 'fs';
import path from 'path';

const assetsDir = path.resolve('../assets');

// Find the built files
const files = fs.readdirSync(path.join(assetsDir, 'assets'));
const cssFile = files.find(f => f.endsWith('.css'));
const jsFile = files.find(f => f.endsWith('.js') && !f.includes('legacy'));

if (!cssFile || !jsFile) {
  console.error('Could not find CSS or JS files');
  process.exit(1);
}

// Read the files
const css = fs.readFileSync(path.join(assetsDir, 'assets', cssFile), 'utf-8');
const js = fs.readFileSync(path.join(assetsDir, 'assets', jsFile), 'utf-8');

// Create single HTML file
const html = `<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Manta AI Terminal</title>
    <style>${css}</style>
</head>
<body>
    <div id="root"></div>
    <script type="module">${js}</script>
</body>
</html>`;

// Write the bundled file
fs.writeFileSync(path.join(assetsDir, 'web_terminal.html'), html);

// Clean up separate files
fs.rmSync(path.join(assetsDir, 'assets'), { recursive: true });
fs.rmSync(path.join(assetsDir, 'index.html'));

console.log('✓ Created web_terminal.html (single file bundle)');
