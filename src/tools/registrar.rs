//! [`ToolRegistrar`]: validated dynamic tool registration.

use std::collections::HashMap;

use serde_json::Value;

use super::{
    BoxedTool, NameValidator, SchemaValidator, SecurityValidator, SharedTool, ToolContext,
    ToolExecutionResult, ToolRegistry, ToolValidationError, ToolValidator,
};
use crate::providers::FunctionDefinition;

/// ToolRegistrar for dynamic tool registration with validation
#[derive(Debug, Default)]
pub struct ToolRegistrar {
    registry: ToolRegistry,
    validators: Vec<Box<dyn ToolValidator>>,
}

impl ToolRegistrar {
    /// Create a new ToolRegistrar with default validators
    pub fn new() -> Self {
        Self {
            registry: ToolRegistry::new(),
            validators: vec![
                Box::new(NameValidator),
                Box::new(SchemaValidator),
                Box::new(SecurityValidator),
            ],
        }
    }

    /// Create with custom validators
    pub fn with_validators(validators: Vec<Box<dyn ToolValidator>>) -> Self {
        Self {
            registry: ToolRegistry::new(),
            validators,
        }
    }

    /// Register a tool with validation
    pub fn register(&mut self, tool: BoxedTool) -> Result<(), ToolValidationError> {
        // Run all validators
        for validator in &self.validators {
            validator.validate(tool.as_ref())?;
        }

        self.registry.register(tool);
        Ok(())
    }

    /// Validate tool input before execution
    pub fn validate_input(&self, tool_name: &str, args: &Value) -> Result<(), ToolValidationError> {
        for validator in &self.validators {
            validator.validate_input(tool_name, args)?;
        }
        Ok(())
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<SharedTool> {
        self.registry.get(name)
    }

    /// List available tool names
    pub fn list(&self) -> Vec<String> {
        self.registry.list()
    }

    /// Check if a tool exists
    pub fn has(&self, name: &str) -> bool {
        self.registry.has(name)
    }

    /// Get tool descriptions
    pub fn get_descriptions(&self) -> HashMap<String, String> {
        self.registry
            .list()
            .into_iter()
            .filter_map(|name| {
                self.registry
                    .get(&name)
                    .map(|t| (name.clone(), t.description().to_string()))
            })
            .collect()
    }

    /// Execute a tool with validation
    pub async fn execute(
        &self,
        name: &str,
        args: Value,
        context: &ToolContext,
    ) -> Option<crate::Result<ToolExecutionResult>> {
        // Validate input first
        if let Err(e) = self.validate_input(name, &args) {
            return Some(Err(crate::error::SyscityError::Validation(e.to_string())));
        }

        self.registry.execute(name, args, context).await
    }

    /// Get all tools as function definitions
    pub fn get_definitions(&self) -> Vec<FunctionDefinition> {
        self.registry.get_definitions()
    }

    /// Add a custom validator
    pub fn add_validator(&mut self, validator: Box<dyn ToolValidator>) {
        self.validators.push(validator);
    }

    /// Get reference to inner registry
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}
