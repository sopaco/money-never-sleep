MNS\distribution\skill\mns-cli\scripts\detect-platform.js
```javascript
#!/usr/bin/env node

/**
 * MNS CLI Platform Detection Script
 *
 * Detects the current platform (OS + architecture) and outputs:
 * - platform ID (e.g., "darwin-arm64", "linux-x64", "win-x64")
 * - npm package name (e.g., "@never-sleeps/cli-darwin-arm64")
 *
 * Usage:
 *   node detect-platform.js          # outputs platform ID
 *   node detect-platform.js platform # outputs platform ID (same as above)
 *   node detect-platform.js npm      # outputs npm package name
 *   node detect-platform.js json     # outputs JSON with both values
 *
 * In agent skills, use:
 *   const platform = require('./detect-platform.js')('platform');
 *   // then run: npx @never-sleeps/cli-${platform} ...
 */

const os = require('os');

function detectPlatform() {
  const platform = os.platform();
  const arch = os.arch();

  // Map Node.js arch names to our package arch names
  const archMap = {
    'x64': 'x64',
    'arm64': 'arm64',
    'ia32': 'ia32'
  };

  let osName;
  switch (platform) {
    case 'darwin':
      osName = 'darwin';
      break;
    case 'linux':
      osName = 'linux';
      break;
    case 'win32':
      osName = 'win';
      break;
    default:
      console.error(`Unsupported platform: ${platform}`);
      process.exit(1);
  }

  const archName = archMap[arch];
  if (!archName) {
    console.error(`Unsupported architecture: ${arch}`);
    process.exit(1);
  }

  return {
    platform: `${osName}-${archName}`,
    npmPackage: `@never-sleeps/cli-${osName}-${archName}`
  };
}

// Determine output mode
const args = process.argv.slice(2);
const mode = args[0] || 'platform';

const result = detectPlatform();

// Output based on mode
if (mode === 'platform') {
  console.log(result.platform);
} else if (mode === 'npm') {
  console.log(result.npmPackage);
} else if (mode === 'json') {
  console.log(JSON.stringify(result));
} else {
  console.error(`Unknown mode: ${mode}. Use 'platform', 'npm', or 'json'`);
  process.exit(1);
}
