# `@key(course, student)` is a composite primary key over two declared columns — no
# surrogate `id`. The key parts are to-one relations, so their FK columns form the PK. A
# relation *into* the composite-key model is a multi-column FK auto-expanded to
# `<field>_<part>` columns, and its structured id projects bare as a per-part object.
Course {
  id:    Id
  title: text
}

Student {
  id:   Id
  name: text
}

@key(course, student)
Enrollment {
  course:   Course
  student:  Student
  grade:    int
  sessions: Session[]
}

@sort(id asc)
Session {
  id:         Id
  enrollment: Enrollment
  note:       text
  @index enrollment
}

shape EnrollmentRow from Enrollment { course = course.title, student = student.name, grade }
shape SessionRow from Session { id, note, enrollment }

query enrollment(course, student) -> EnrollmentRow;
query enrollment_sessions(course, student) -> EnrollmentWithSessions;

shape EnrollmentWithSessions from Enrollment { grade, sessions { note } }
