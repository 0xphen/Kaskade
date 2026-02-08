DB_DIR := backend/dev
DB_FILE := $(DB_DIR)/kaskade_dev.db
DATABASE_URL := sqlite://$(PWD)/$(DB_FILE)

export DATABASE_URL

.PHONY: help
help:
	@echo ""
	@echo "Database commands:"
	@echo "  make db-reset      Delete DB, recreate, run migrations"
	@echo "  make db-migrate    Run migrations only"
	@echo "  make db-seed       Run seed SQL (dev)"
	@echo "  make db-shell      Open sqlite shell"
	@echo ""

.PHONY: db-reset
db-reset:
	@echo "🧨 Resetting dev database"
	rm -rf $(DB_DIR)
	mkdir -p $(DB_DIR)
	touch $(DB_FILE)
	chmod u+rw $(DB_FILE)
	@echo "DATABASE_URL=$(DATABASE_URL)"
	cargo sqlx migrate run
	@echo "✅ Database reset complete"

.PHONY: db-migrate
db-migrate:
	@echo "📦 Running migrations"
	@echo "DATABASE_URL=$(DATABASE_URL)"
	cargo sqlx migrate run

.PHONY: db-seed
db-seed:
	@echo "🌱 Seeding database"
	sqlite3 $(DB_FILE) < backend/migrations/dev_seed.sql
	@echo "✅ Seed complete"

.PHONY: db-shell
db-shell:
	sqlite3 $(DB_FILE)
