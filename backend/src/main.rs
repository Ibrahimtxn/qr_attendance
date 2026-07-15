use actix_web::{web, App, HttpServer, HttpResponse, Responder, middleware::Logger};
use actix_cors::Cors;
use actix_files::Files;
use serde_json::json;
use chrono::{Utc, Duration, DateTime};
use deadpool_postgres::{Config, Pool, Runtime};
use tokio_postgres::NoTls;
use dotenv::dotenv;
use jsonwebtoken::{encode, Header, EncodingKey};
use md5;
use qrcode::{QrCode, Color};
use image::Luma;
use base64::engine::Engine;

// ─────────────────────────────────────────────────────────────
// MODELS
// ─────────────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct StudentRegister {
    student_id: String,
    full_name: String,
    email: String,
    password: String,
    level: String,
    department: String,
}

#[derive(serde::Deserialize)]
struct LecturerRegister {
    staff_id: String,
    full_name: String,
    email: String,
    password: String,
    department: String,
}

#[derive(serde::Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    role: String,
}

#[derive(serde::Deserialize)]
struct ChangePasswordRequest {
    user_id: i32,
    old_password: String,
    new_password: String,
    role: String,
}

#[derive(serde::Deserialize)]
struct ForgotPasswordRequest {
    email: String,
    new_password: String,
    role: String,
}

// Accepts duration, lecturer GPS, radius and room_size.
// All optional except course_id/lecturer_id — backend falls
// back to sane defaults if any are missing.
#[derive(serde::Deserialize)]
struct GenerateQRRequest {
    course_id: i32,
    lecturer_id: i32,
    duration: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    radius: Option<f64>,
    room_size: Option<String>,
}

#[derive(serde::Deserialize)]
struct ScanQRRequest {
    qr_data: String,
    student_id: i32,
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(serde::Deserialize)]
struct CreateCourseRequest {
    code: String,
    name: String,
    lecturer_id: i32,
    department: String,
    level: String,
    semester: String,
    academic_year: String,
}

#[derive(serde::Deserialize)]
struct DeleteAttendanceRequest {
    course_id: i32,
    semester: String,
}

#[derive(serde::Deserialize)]
struct ReassignCourseRequest {
    lecturer_id: i32,
}

// ─────────────────────────────────────────────────────────────
// ROOM SIZE PRESETS
// Calculated from base classroom 17.07m x 7.92m (135.2 m²).
// Formula: radius = sqrt(area / π) — converts rectangular
// floor area into a circular GPS radius centred on the
// lecturer's position.
//
//  small    : (17.07/2) x 7.92      =  67.6 m²  → 4.6m
//  standard : 17.07 x 7.92          = 135.2 m²  → 6.6m
//  large    : (17.07x2.5) x (7.92x2.5) = 845.1 m² → 16.4m
// ─────────────────────────────────────────────────────────────

const RADIUS_SMALL: f64    = 4.6;
const RADIUS_STANDARD: f64 = 6.6;
const RADIUS_LARGE: f64    = 16.4;

fn radius_for_room_size(room_size: &str) -> f64 {
    match room_size {
        "small" => RADIUS_SMALL,
        "large" => RADIUS_LARGE,
        _       => RADIUS_STANDARD,
    }
}

// ─────────────────────────────────────────────────────────────
// HELPERS
// ─────────────────────────────────────────────────────────────

fn create_token(user_id: i32, role: &str, email: &str) -> String {
    let claims = json!({
        "sub": user_id,
        "role": role,
        "email": email,
        "exp": (Utc::now() + Duration::hours(24)).timestamp()
    });
    encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret("secret".as_ref()),
    )
    .unwrap()
}

fn generate_qr_image(data: &str) -> String {
    let code = QrCode::new(data).unwrap();
    let size = code.width() as u32;
    let mut img = image::ImageBuffer::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let pixel = if code[(x as usize, y as usize)] == Color::Light { 255u8 } else { 0u8 };
            img.put_pixel(x, y, Luma([pixel]));
        }
    }
    let mut buffer = std::io::Cursor::new(Vec::new());
    img.write_to(&mut buffer, image::ImageFormat::Png).unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(buffer.into_inner())
    )
}

// Haversine formula — great-circle distance in metres between
// two GPS coordinate pairs.
fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6_371_000.0_f64;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    r * c
}

async fn create_pool() -> Pool {
    let mut config = Config::new();
    config.host     = Some("localhost".to_string());
    config.dbname   = Some("attendance_db".to_string());
    config.user     = Some("postgres".to_string());
    config.password = Some("postgres".to_string());
    config.create_pool(Some(Runtime::Tokio1), NoTls).unwrap()
}

// ─────────────────────────────────────────────────────────────
// AUTH HANDLERS
// ─────────────────────────────────────────────────────────────

async fn login(req: web::Json<LoginRequest>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let hash   = format!("{:x}", md5::compute(&req.password));

    let row = match req.role.as_str() {
        "student" => client.query_one(
            "SELECT id, full_name, student_id FROM students WHERE email = $1 AND password_hash = $2",
            &[&req.email, &hash],
        ).await,
        "lecturer" => client.query_one(
            "SELECT id, full_name, staff_id FROM lecturers WHERE email = $1 AND password_hash = $2",
            &[&req.email, &hash],
        ).await,
        "admin" => client.query_one(
            "SELECT id, full_name, username FROM admin WHERE username = $1 AND password_hash = $2",
            &[&req.email, &hash],
        ).await,
        _ => return HttpResponse::BadRequest().json(json!({"error": "Invalid role"})),
    };

    match row {
        Ok(row) => {
            let user_id:   i32    = row.get(0);
            let full_name: String = row.get(1);
            HttpResponse::Ok().json(json!({
                "token":     create_token(user_id, &req.role, &req.email),
                "user_id":   user_id,
                "role":      req.role,
                "full_name": full_name
            }))
        }
        Err(_) => HttpResponse::Unauthorized().json(json!({"error": "Invalid credentials"})),
    }
}

async fn register_student(req: web::Json<StudentRegister>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let hash   = format!("{:x}", md5::compute(&req.password));
    match client.execute(
        "INSERT INTO students (student_id, full_name, email, password_hash, level, department)
         VALUES ($1, $2, $3, $4, $5, $6)",
        &[&req.student_id, &req.full_name, &req.email, &hash, &req.level, &req.department],
    ).await {
        Ok(_)  => HttpResponse::Ok().json(json!({"success": true, "message": "Student registered"})),
        Err(_) => HttpResponse::BadRequest().json(json!({"error": "Email or Student ID already exists"})),
    }
}

async fn register_lecturer(req: web::Json<LecturerRegister>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let hash   = format!("{:x}", md5::compute(&req.password));
    match client.execute(
        "INSERT INTO lecturers (staff_id, full_name, email, password_hash, department)
         VALUES ($1, $2, $3, $4, $5)",
        &[&req.staff_id, &req.full_name, &req.email, &hash, &req.department],
    ).await {
        Ok(_)  => HttpResponse::Ok().json(json!({"success": true, "message": "Lecturer registered"})),
        Err(_) => HttpResponse::BadRequest().json(json!({"error": "Email or Staff ID already exists"})),
    }
}

async fn change_password(req: web::Json<ChangePasswordRequest>) -> impl Responder {
    let pool     = create_pool().await;
    let client   = pool.get().await.unwrap();
    let old_hash = format!("{:x}", md5::compute(&req.old_password));
    let new_hash = format!("{:x}", md5::compute(&req.new_password));

    let table = match req.role.as_str() {
        "student"  => "students",
        "lecturer" => "lecturers",
        "admin"    => "admin",
        _          => return HttpResponse::BadRequest().json(json!({"error": "Invalid role"})),
    };

    let updated = client.execute(
        &format!("UPDATE {} SET password_hash = $1 WHERE id = $2 AND password_hash = $3", table),
        &[&new_hash, &req.user_id, &old_hash],
    ).await.unwrap();

    if updated > 0 {
        HttpResponse::Ok().json(json!({"success": true}))
    } else {
        HttpResponse::BadRequest().json(json!({"error": "Invalid old password"}))
    }
}

async fn forgot_password(req: web::Json<ForgotPasswordRequest>) -> impl Responder {
    let pool     = create_pool().await;
    let client   = pool.get().await.unwrap();
    let new_hash = format!("{:x}", md5::compute(&req.new_password));

    let table = match req.role.as_str() {
        "student"  => "students",
        "lecturer" => "lecturers",
        "admin"    => "admin",
        _          => return HttpResponse::BadRequest().json(json!({"error": "Invalid role"})),
    };

    let updated = client.execute(
        &format!("UPDATE {} SET password_hash = $1 WHERE email = $2", table),
        &[&new_hash, &req.email],
    ).await.unwrap();

    if updated > 0 {
        HttpResponse::Ok().json(json!({"success": true, "message": "Password reset successful"}))
    } else {
        HttpResponse::BadRequest().json(json!({"error": "Email not found"}))
    }
}

// ─────────────────────────────────────────────────────────────
// USER / COURSE HANDLERS
// ─────────────────────────────────────────────────────────────

async fn get_all_students() -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let rows   = client.query(
        "SELECT id, student_id, full_name, email, level, department, created_at::text FROM students",
        &[],
    ).await.unwrap();

    let students: Vec<_> = rows.iter().map(|row| json!({
        "id":         row.get::<_, i32>(0),
        "student_id": row.get::<_, String>(1),
        "full_name":  row.get::<_, String>(2),
        "email":      row.get::<_, String>(3),
        "level":      row.get::<_, String>(4),
        "department": row.get::<_, String>(5),
        "created_at": row.get::<_, String>(6)
    })).collect();
    HttpResponse::Ok().json(students)
}

// Returns numeric id — required by admin dashboard lecturer select
async fn get_all_lecturers() -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let rows   = client.query(
        "SELECT id, staff_id, full_name, email, department, created_at::text FROM lecturers",
        &[],
    ).await.unwrap();

    let lecturers: Vec<_> = rows.iter().map(|row| json!({
        "id":         row.get::<_, i32>(0),
        "staff_id":   row.get::<_, String>(1),
        "full_name":  row.get::<_, String>(2),
        "email":      row.get::<_, String>(3),
        "department": row.get::<_, String>(4),
        "created_at": row.get::<_, String>(5)
    })).collect();
    HttpResponse::Ok().json(lecturers)
}

async fn get_all_courses() -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let rows   = client.query(
        "SELECT c.id, c.code, c.name, c.department, c.level, c.semester, c.academic_year,
                l.full_name AS lecturer_name
         FROM courses c
         JOIN lecturers l ON c.lecturer_id = l.id",
        &[],
    ).await.unwrap();

    let courses: Vec<_> = rows.iter().map(|row| json!({
        "id":            row.get::<_, i32>("id"),
        "code":          row.get::<_, String>("code"),
        "name":          row.get::<_, String>("name"),
        "department":    row.get::<_, String>("department"),
        "level":         row.get::<_, String>("level"),
        "semester":      row.get::<_, String>("semester"),
        "academic_year": row.get::<_, String>("academic_year"),
        "lecturer":      row.get::<_, String>("lecturer_name")
    })).collect();
    HttpResponse::Ok().json(courses)
}

async fn get_my_courses(user_id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let rows   = client.query(
        "SELECT id, code, name, department, level, semester, academic_year
         FROM courses WHERE lecturer_id = $1",
        &[&user_id.into_inner()],
    ).await.unwrap();

    let courses: Vec<_> = rows.iter().map(|row| json!({
        "id":            row.get::<_, i32>(0),
        "code":          row.get::<_, String>(1),
        "name":          row.get::<_, String>(2),
        "department":    row.get::<_, String>(3),
        "level":         row.get::<_, String>(4),
        "semester":      row.get::<_, String>(5),
        "academic_year": row.get::<_, String>(6)
    })).collect();
    HttpResponse::Ok().json(courses)
}

async fn create_course(req: web::Json<CreateCourseRequest>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    match client.execute(
        "INSERT INTO courses (code, name, lecturer_id, department, level, semester, academic_year)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
        &[&req.code, &req.name, &req.lecturer_id, &req.department, &req.level, &req.semester, &req.academic_year],
    ).await {
        Ok(_)  => HttpResponse::Ok().json(json!({"success": true})),
        Err(_) => HttpResponse::BadRequest().json(json!({"error": "Course code already exists"})),
    }
}

// ─────────────────────────────────────────────────────────────
// ADMIN CRUD — DELETE / REASSIGN
// ─────────────────────────────────────────────────────────────

async fn delete_student(id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let uid    = id.into_inner();

    // Must remove attendance records first (FK constraint)
    client.execute(
        "DELETE FROM attendance_records WHERE student_id = $1",
        &[&uid],
    ).await.unwrap();

    let deleted = client.execute(
        "DELETE FROM students WHERE id = $1",
        &[&uid],
    ).await.unwrap();

    if deleted > 0 {
        HttpResponse::Ok().json(json!({"success": true, "message": "Student deleted"}))
    } else {
        HttpResponse::NotFound().json(json!({"error": "Student not found"}))
    }
}

// Blocked if lecturer still owns any courses — admin must
// reassign or delete courses first.
async fn delete_lecturer(id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let uid    = id.into_inner();

    let course_count: i64 = client.query_one(
        "SELECT COUNT(*) FROM courses WHERE lecturer_id = $1",
        &[&uid],
    ).await.unwrap().get(0);

    if course_count > 0 {
        return HttpResponse::BadRequest().json(json!({
            "error": format!(
                "Cannot delete: lecturer has {} course(s). Reassign or delete them first.",
                course_count
            )
        }));
    }

    let deleted = client.execute(
        "DELETE FROM lecturers WHERE id = $1",
        &[&uid],
    ).await.unwrap();

    if deleted > 0 {
        HttpResponse::Ok().json(json!({"success": true, "message": "Lecturer deleted"}))
    } else {
        HttpResponse::NotFound().json(json!({"error": "Lecturer not found"}))
    }
}

// Cascades: records → sessions → course (FK order)
async fn delete_course(id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let cid    = id.into_inner();

    client.execute(
        "DELETE FROM attendance_records
         WHERE session_id IN (SELECT id FROM attendance_sessions WHERE course_id = $1)",
        &[&cid],
    ).await.unwrap();

    client.execute(
        "DELETE FROM attendance_sessions WHERE course_id = $1",
        &[&cid],
    ).await.unwrap();

    let deleted = client.execute(
        "DELETE FROM courses WHERE id = $1",
        &[&cid],
    ).await.unwrap();

    if deleted > 0 {
        HttpResponse::Ok().json(json!({"success": true, "message": "Course deleted"}))
    } else {
        HttpResponse::NotFound().json(json!({"error": "Course not found"}))
    }
}

async fn reassign_course(
    course_id: web::Path<i32>,
    req: web::Json<ReassignCourseRequest>,
) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();

    let updated = client.execute(
        "UPDATE courses SET lecturer_id = $1 WHERE id = $2",
        &[&req.lecturer_id, &course_id.into_inner()],
    ).await.unwrap();

    if updated > 0 {
        HttpResponse::Ok().json(json!({"success": true, "message": "Course reassigned"}))
    } else {
        HttpResponse::NotFound().json(json!({"error": "Course not found"}))
    }
}

// ─────────────────────────────────────────────────────────────
// QR HANDLERS
// ─────────────────────────────────────────────────────────────

// Generates a time-expiring QR code and saves the lecturer's
// live GPS coordinates + chosen room size radius to the session
// row — no global classroom constant anywhere.
async fn generate_qr(req: web::Json<GenerateQRRequest>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();

    // Verify lecturer owns this course
    let check = client.query_one(
        "SELECT id FROM courses WHERE id = $1 AND lecturer_id = $2",
        &[&req.course_id, &req.lecturer_id],
    ).await;

    if check.is_err() {
        return HttpResponse::Forbidden().json(json!({"error": "You don't own this course"}));
    }

    let duration   = req.duration.unwrap_or(60);
    let expires_at = Utc::now() + Duration::seconds(duration);
    let expires_ts = expires_at.timestamp();

    // Token format: ATTENDANCE:<course_id>:<unix_expiry>
    let qr_string = format!("ATTENDANCE:{}:{}", req.course_id, expires_ts);

    // Radius: explicit value wins, then room_size preset, then standard default
    let room_size = req.room_size.clone().unwrap_or_else(|| "standard".to_string());
    let radius    = req.radius.unwrap_or_else(|| radius_for_room_size(&room_size));

    client.execute(
        "INSERT INTO attendance_sessions
         (course_id, lecturer_id, qr_code, expires_at, latitude, longitude, radius, room_size)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[
            &req.course_id, &req.lecturer_id, &qr_string,
            &expires_ts, &req.latitude, &req.longitude,
            &radius, &room_size,
        ],
    ).await.unwrap();

    let qr_image = generate_qr_image(&qr_string);

    HttpResponse::Ok().json(json!({
        "qr_image":          qr_image,
        "qr_text":           qr_string,
        "expires_at":        expires_at,
        "course_id":         req.course_id,
        "location_captured": req.latitude.is_some(),
        "radius":            radius,
        "room_size":         room_size
    }))
}

// Validates the scanned QR and records attendance.
// Distance check uses the SESSION's own stored lecturer GPS
// and radius — not any global constant.
async fn scan_qr(req: web::Json<ScanQRRequest>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();

    let parts: Vec<&str> = req.qr_data.split(':').collect();
    if parts.len() != 3 || parts[0] != "ATTENDANCE" {
        return HttpResponse::BadRequest()
            .json(json!({"success": false, "message": "Invalid QR code format"}));
    }

    let course_id:  i32 = parts[1].parse().unwrap();
    let expires_ts: i64 = parts[2].parse().unwrap();
    let now = Utc::now().timestamp();

    if now > expires_ts {
        return HttpResponse::BadRequest()
            .json(json!({"success": false, "message": "QR code has expired"}));
    }

    // Pull session with its GPS + radius — self-contained per session
    let session = client.query_one(
        "SELECT id, latitude, longitude, radius
         FROM attendance_sessions
         WHERE course_id = $1 AND expires_at = $2
         ORDER BY id DESC LIMIT 1",
        &[&course_id, &expires_ts],
    ).await;

    let (session_id, session_lat, session_lng, session_radius):
        (i32, Option<f64>, Option<f64>, Option<f64>) = match session {
        Ok(row) => (row.get(0), row.get(1), row.get(2), row.get(3)),
        Err(_)  => return HttpResponse::BadRequest()
            .json(json!({"success": false, "message": "Session not found"})),
    };

    // Duplicate check — enforced at DB level too (UNIQUE constraint)
    let existing: i64 = client.query_one(
        "SELECT COUNT(*) FROM attendance_records WHERE session_id = $1 AND student_id = $2",
        &[&session_id, &req.student_id],
    ).await.unwrap().get(0);

    if existing > 0 {
        return HttpResponse::BadRequest()
            .json(json!({"success": false, "message": "Attendance already marked for this session"}));
    }

    // GPS validation — only runs if lecturer captured location
    if let (Some(s_lat), Some(s_lng)) = (session_lat, session_lng) {
        if let (Some(lat), Some(lng)) = (req.latitude, req.longitude) {
            let radius   = session_radius.unwrap_or(RADIUS_STANDARD);
            let distance = haversine_distance(lat, lng, s_lat, s_lng);
            if distance > radius {
                return HttpResponse::BadRequest().json(json!({
                    "success": false,
                    "message": format!(
                        "You are {:.1}m away from the lecturer. Must be within {:.1}m.",
                        distance, radius
                    )
                }));
            }
        }
        // Uncomment below to strictly require student GPS when
        // lecturer shared theirs:
        // else {
        //     return HttpResponse::BadRequest()
        //         .json(json!({"success": false, "message": "Location permission required"}));
        // }
    }

    client.execute(
        "INSERT INTO attendance_records (session_id, student_id, scanned_at, latitude, longitude)
         VALUES ($1, $2, $3, $4, $5)",
        &[&session_id, &req.student_id, &now, &req.latitude, &req.longitude],
    ).await.unwrap();

    HttpResponse::Ok().json(json!({
        "success":   true,
        "message":   "Attendance marked successfully",
        "timestamp": Utc::now().to_rfc3339()
    }))
}

// ─────────────────────────────────────────────────────────────
// REPORT / SESSION HANDLERS
// ─────────────────────────────────────────────────────────────

async fn get_attendance_report(course_id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let cid    = course_id.into_inner();

    let rows = client.query(
        "SELECT s.student_id, s.full_name, s.level,
                COUNT(ar.id) AS attended,
                (SELECT COUNT(*) FROM attendance_sessions WHERE course_id = $1) AS total_sessions,
                MAX(ar.scanned_at) AS last_attendance
         FROM students s
         LEFT JOIN attendance_records ar ON ar.student_id = s.id
         LEFT JOIN attendance_sessions ses
               ON ar.session_id = ses.id AND ses.course_id = $1
         GROUP BY s.id, s.student_id, s.full_name, s.level",
        &[&cid],
    ).await.unwrap();

    let report: Vec<_> = rows.iter().map(|row| json!({
        "student_id":    row.get::<_, String>("student_id"),
        "student_name":  row.get::<_, String>("full_name"),
        "level":         row.get::<_, String>("level"),
        "attended":      row.get::<_, i64>("attended"),
        "total_sessions":row.get::<_, i64>("total_sessions"),
        "last_attendance": row.get::<_, Option<i64>>("last_attendance").map(|ts| {
            DateTime::from_timestamp(ts, 0)
                .unwrap()
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
    })).collect();

    HttpResponse::Ok().json(report)
}

// Returns last 10 sessions with room_size and radius included
async fn get_course_sessions(course_id: web::Path<i32>) -> impl Responder {
    let pool   = create_pool().await;
    let client = pool.get().await.unwrap();
    let rows   = client.query(
        "SELECT id, created_at::text, expires_at, room_size, radius
         FROM attendance_sessions
         WHERE course_id = $1
         ORDER BY created_at DESC LIMIT 10",
        &[&course_id.into_inner()],
    ).await.unwrap();

    let sessions: Vec<_> = rows.iter().map(|row| json!({
        "session_id": row.get::<_, i32>("id"),
        "created_at": row.get::<_, String>("created_at"),
        "expires_at": DateTime::from_timestamp(row.get::<_, i64>("expires_at"), 0)
            .unwrap()
            .format("%Y-%m-%d %H:%M:%S")
            .to_string(),
        "room_size": row.get::<_, String>("room_size"),
        "radius":    row.get::<_, f64>("radius")
    })).collect();

    HttpResponse::Ok().json(sessions)
}

async fn delete_attendance_records(req: web::Json<DeleteAttendanceRequest>) -> impl Responder {
    let pool    = create_pool().await;
    let client  = pool.get().await.unwrap();
    let deleted = client.execute(
        "DELETE FROM attendance_records
         WHERE session_id IN (
             SELECT id FROM attendance_sessions WHERE course_id = $1
         )",
        &[&req.course_id],
    ).await.unwrap();

    HttpResponse::Ok().json(json!({
        "success":         true,
        "deleted_records": deleted,
        "message":         format!("Deleted {} attendance records", deleted)
    }))
}

// ─────────────────────────────────────────────────────────────
// MAIN
// ─────────────────────────────────────────────────────────────

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenv().ok();
    env_logger::init();

    println!("🚀 Server running at http://localhost:8080");
    println!(
        "📐 Geofence radii — small: {}m · standard: {}m · large: {}m",
        RADIUS_SMALL, RADIUS_STANDARD, RADIUS_LARGE
    );

    HttpServer::new(move || {
        App::new()
            .wrap(Cors::permissive())
            .wrap(Logger::default())

            // Health check
            .route("/health", web::get().to(|| async { HttpResponse::Ok().body("OK") }))

            // ── Auth ───────────────────────────────────────────
            .route("/api/auth/login",              web::post().to(login))
            .route("/api/auth/register-student",   web::post().to(register_student))
            .route("/api/auth/register-lecturer",  web::post().to(register_lecturer))
            .route("/api/auth/change-password",    web::post().to(change_password))
            .route("/api/auth/forgot-password",    web::post().to(forgot_password))

            // ── Users ──────────────────────────────────────────
            .route("/api/students",                web::get().to(get_all_students))
            .route("/api/students/{id}",           web::delete().to(delete_student))
            .route("/api/lecturers",               web::get().to(get_all_lecturers))
            .route("/api/lecturers/{id}",          web::delete().to(delete_lecturer))

            // ── Courses ────────────────────────────────────────
            .route("/api/courses",                 web::get().to(get_all_courses))
            .route("/api/courses/my/{user_id}",    web::get().to(get_my_courses))
            .route("/api/courses/create",          web::post().to(create_course))
            .route("/api/courses/{id}",            web::delete().to(delete_course))
            .route("/api/courses/{id}/reassign",   web::put().to(reassign_course))
            .route("/api/courses/{id}/sessions",   web::get().to(get_course_sessions))

            // ── QR ─────────────────────────────────────────────
            .route("/api/qr/generate",             web::post().to(generate_qr))
            .route("/api/qr/scan",                 web::post().to(scan_qr))

            // ── Reports ────────────────────────────────────────
            .route("/api/reports/{course_id}",     web::get().to(get_attendance_report))
            .route("/api/attendance/delete",       web::post().to(delete_attendance_records))

            // ── Static files (frontend) ────────────────────────
            // Serves everything in ./frontend — must be last
            .service(Files::new("/", "../frontend").index_file("index.html"))
    })
    .bind("0.0.0.0:8080")?
    .run()
    .await
}
