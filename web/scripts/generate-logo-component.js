import fs from 'fs';
import path from 'path';

const pngPath = path.resolve('../assets/manta.png');
const outputPath = path.resolve('src/components/MantaLogo.tsx');

// Read PNG and convert to base64
const pngBuffer = fs.readFileSync(pngPath);
const base64 = pngBuffer.toString('base64');
const dataUri = `data:image/png;base64,${base64}`;

// Generate component
const component = `// Auto-generated from assets/manta.png
// To regenerate: node scripts/generate-logo-component.js

export function MantaLogo() {
  return (
    <img
      src="${dataUri}"
      alt="Manta Logo"
      className="logo"
      width="28"
      height="28"
    />
  );
}
`;

fs.writeFileSync(outputPath, component);
console.log(`✓ Generated MantaLogo.tsx (${(base64.length / 1024).toFixed(1)}KB base64)`);
