User {
  id:    Id
  name:  text
  email: text
  bio:   text
}

shape UserBase from User {
  id
  name
  email
}

shape UserCard from User {
  ...UserBase
  bio
}

query get_user(id) -> UserCard;
