#!/usr/bin/env node

/**
 * MNS CLI Runner for Agent Skills
 *
 * A wrapper script that provides a unified interface to run MNS CLI commands
 * regardless of the platform. This script automatically detects the platform
 * and invokes the appropriate prebuilt binary via npx.
 *
 * Usage:
 *   node run-mns.js <command> [args...]
 *
 * Examples:
 *   node run-mns.js portfolio
 *   node run-mns.js report
 *   node run-mns.js config thresholds.fear
 *
 * In agent skills, use this script as the entry point to avoid platform detection
 * logic in every skill invocation.
 */

const { spawn } = require('child_process');
const path = require('path');
const os = require('os');

function detectPlatform() {
  const platform = os.platform();
  const arch = os.arch();

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
      return { error: `Unsupported platform: ${platform}` };
  }

  const archName = archMap[arch];
  if (!archName) {
    return { error: `Unsupported architecture: ${arch}` };
  }

  return {
    platform: `${osName}-${archName}`,
    npmPackage: `@mns/cli-${osName}-${archName}`
  };
}

function runMnsCommand(args) {
  if (args.length === 0) {
    console.error('Usage: run-mns.js <command> [args...]');
    console.error('Example: run-mns.js portfolio');
    process.exit(1);
  }

  const command = args[0];
  const commandArgs = args.slice(1);

  const platformInfo = detectPlatform();

  if (platformInfo.error) {
    console.error(`Error: ${platformInfo.error}`);
    process.exit(1);
  }

  const npmPackage = platformInfo.npmPackage;
  const mnsArgs = [npmPackage, command, ...commandArgs];

  // Use npx to run the prebuilt binary
  // We use npx with --yes to avoid prompts, and --ignore-scripts to prevent running install scripts
  // The binary will be extracted from the package and executed directly
  const npxArgs = [
    '--yes',
    '--ignore-scripts',
    ...mnsArgs
  ];

  const child = spawn('npx', npxArgs, {
    stdio: 'inherit',
    shell: true, // Use shell for npx to work properly on all platforms
  });

  child.on('error', (err) => {
    console.error(`Failed to execute npx: ${err.message}`);
    console.error('\nMake sure Node.js and npx are available in PATH.');
    console.error('If npx is not available, you can install the package manually:');
    console.error(`  npm install -g ${npmPackage}`);
    process.exit(1);
  });

  child.on('close', (code) => {
    process.exit(code || 0);
  });
}

// Run the command
runMnsCommand(process.argv.slice(2));
