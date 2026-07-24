# Many-to-many far-side flattening projection: `courses = enrollments.course
# { title }` hides the junction (Enrollment) and returns the distinct set of
# far-side Course rows as a flat `Vec<Course>`. The first path segment is a
# to-many inverse edge (into the junction); the rest are forward edges to the
# far model, whose projection is the brace body.
Student {
  id:          Id
  name:        text
  enrollments: Enrollment[] (Enrollment.student)
}

Enrollment {
  id:      Id
  student: Student
  course:  Course
  @index (student, course) unique
}

Course {
  id:    Id
  title: text
}

shape StudentCourses from Student {
  name
  courses = enrollments.course { title }
}

query student_courses() -> StudentCourses[] {
  list Student order (name);
}
