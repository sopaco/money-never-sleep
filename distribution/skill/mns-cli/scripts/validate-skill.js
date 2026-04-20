#!/usr/bin/env node

/**
 * MNS Skill Validator
 * Validates the skill structure against agentskills.io spec
 */

const fs = require("fs");
const path = require("path");

const SKILL_DIR = path.join(__dirname, "..");

function checkFile(filePath, description) {
  const fullPath = path.join(SKILL_DIR, filePath);
  if (!fs.existsSync(fullPath)) {
    console.error(`✗ Missing ${description}: ${filePath}`);
    return false;
  }
  console.log(`✓ Found ${description}: ${filePath}`);
  return true;
}

function validateSkill() {
  console.log("Validating MNS CLI Skill...\n");

  let errors = 0;

  // Required: SKILL.md
  if (!checkFile("SKILL.md", "skill manifest")) {
    errors++;
    return errors;
  }

  // Validate SKILL.md frontmatter
  const skillPath = path.join(SKILL_DIR, "SKILL.md");
  const content = fs.readFileSync(skillPath, "utf8");

  // Check frontmatter exists
  if (!content.startsWith("---")) {
    console.error("✗ SKILL.md missing YAML frontmatter");
    errors++;
  } else {
    console.log("✓ SKILL.md has frontmatter");
  }

  // Check for required fields
  const requiredFields = ["name:", "description:"];
  for (const field of requiredFields) {
    if (!content.includes(field)) {
      console.error(`✗ Missing required field: ${field}`);
      errors++;
    } else {
      console.log(`✓ Found required field: ${field}`);
    }
  }

  // Check name format (lowercase, hyphens only)
  const nameMatch = content.match(/^name:\s*(.+)$/m);
  if (nameMatch) {
    const name = nameMatch[1].trim();
    if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(name)) {
      console.error(
        `✗ Invalid skill name: ${name}. Must be lowercase with hyphens.`,
      );
      errors++;
    } else {
      console.log(`✓ Valid skill name: ${name}`);
    }
  }

  // Optional directories (should exist if referenced)
  console.log("\nChecking optional directories:");
  checkFile("scripts/detect-platform.js", "platform detector");
  checkFile("scripts/run-mns.js", "command runner");
  checkFile("references/commands.md", "command reference");
  checkFile("references/strategy.md", "strategy reference");
  checkFile("assets/examples/portfolio-example.json", "portfolio example");
  checkFile("assets/examples/report-example.json", "report example");

  console.log("\n" + "=".repeat(50));
  if (errors === 0) {
    console.log("✓ Skill validation passed!");
  } else {
    console.log(`✗ Skill validation failed with ${errors} error(s)`);
  }

  return errors;
}

const exitCode = validateSkill();
process.exit(exitCode > 0 ? 1 : 0);
