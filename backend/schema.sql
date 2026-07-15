-- ============================================================
-- QR Code Attendance System — Database Schema
-- PostgreSQL
-- ============================================================

-- Drop existing tables (clean rebuild)
DROP TABLE IF EXISTS attendance_records CASCADE;
DROP TABLE IF EXISTS attendance_sessions CASCADE;
DROP TABLE IF EXISTS courses CASCADE;
DROP TABLE IF EXISTS students CASCADE;
DROP TABLE IF EXISTS lecturers CASCADE;
DROP TABLE IF EXISTS admin CASCADE;

-- ── Students ──────────────────────────────────────────────────
CREATE TABLE students (
    id SERIAL PRIMARY KEY,
    student_id VARCHAR(20) UNIQUE NOT NULL,
    full_name VARCHAR(100) NOT NULL,
    email VARCHAR(100) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    level VARCHAR(20) NOT NULL,
    department VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ── Lecturers ─────────────────────────────────────────────────
CREATE TABLE lecturers (
    id SERIAL PRIMARY KEY,
    staff_id VARCHAR(20) UNIQUE NOT NULL,
    full_name VARCHAR(100) NOT NULL,
    email VARCHAR(100) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    department VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ── Admin ─────────────────────────────────────────────────────
CREATE TABLE admin (
    id SERIAL PRIMARY KEY,
    username VARCHAR(50) UNIQUE NOT NULL,
    password_hash VARCHAR(255) NOT NULL,
    full_name VARCHAR(100) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ── Courses ───────────────────────────────────────────────────
CREATE TABLE courses (
    id SERIAL PRIMARY KEY,
    code VARCHAR(20) UNIQUE NOT NULL,
    name VARCHAR(100) NOT NULL,
    lecturer_id INTEGER REFERENCES lecturers(id),
    department VARCHAR(100) NOT NULL,
    level VARCHAR(20) NOT NULL,
    semester VARCHAR(20) NOT NULL,
    academic_year VARCHAR(20) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ── Attendance Sessions ───────────────────────────────────────
-- Each QR generation event. Stores the LECTURER'S live GPS position
-- and the chosen geofence radius at the moment of generation, so
-- every session is self-contained (no hardcoded classroom constant).
CREATE TABLE attendance_sessions (
    id SERIAL PRIMARY KEY,
    course_id INTEGER REFERENCES courses(id),
    lecturer_id INTEGER REFERENCES lecturers(id),
    qr_code TEXT NOT NULL,
    expires_at BIGINT NOT NULL,
    latitude DOUBLE PRECISION,
    longitude DOUBLE PRECISION,
    radius DOUBLE PRECISION DEFAULT 6.6,
    room_size VARCHAR(20) DEFAULT 'standard',
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- ── Attendance Records ────────────────────────────────────────
CREATE TABLE attendance_records (
    id SERIAL PRIMARY KEY,
    session_id INTEGER REFERENCES attendance_sessions(id),
    student_id INTEGER REFERENCES students(id),
    scanned_at BIGINT NOT NULL,
    latitude DOUBLE PRECISION,
    longitude DOUBLE PRECISION,
    UNIQUE(session_id, student_id)
);

-- ============================================================
-- Room size → geofence radius reference
-- Base classroom: 17.07m x 7.92m
-- ============================================================
-- small     : (17.07/2) x 7.92  = 67.6 m²   -> radius = sqrt(area/pi) = 4.6m
-- standard  : 17.07 x 7.92      = 135.2 m²  -> radius = sqrt(area/pi) = 6.6m
-- large     : (17.07x2.5) x (7.92x2.5) = 845.1 m² -> radius = sqrt(area/pi) = 16.4m
-- ============================================================

-- ── Seed data ─────────────────────────────────────────────────
INSERT INTO admin (username, password_hash, full_name) VALUES
('admin', '482c811da5d5b4bc6d497ffa98491e38', 'System Administrator');

INSERT INTO lecturers (staff_id, full_name, email, password_hash, department) VALUES
('LEC001', 'Dr. John Smith', 'john.smith@university.edu', '482c811da5d5b4bc6d497ffa98491e38', 'Computer Science');

INSERT INTO students (student_id, full_name, email, password_hash, level, department) VALUES
('2024/CS/001', 'Alice Johnson', 'alice@university.edu', '482c811da5d5b4bc6d497ffa98491e38', '300 Level', 'Computer Science');

INSERT INTO courses (code, name, lecturer_id, department, level, semester, academic_year) VALUES
('CS301', 'Software Engineering', 1, 'Computer Science', '300 Level', 'First Semester', '2024/2025');
